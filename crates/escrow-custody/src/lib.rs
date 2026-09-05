//! 預かりの見張り（#15 のスライス）。
//!
//! `holding` の項目を1件受け取り、配信元と突き合わせる。期限まで在り続けたものを
//! 捨て、消えたものは手元に残す（#1）。終端に達した項目の実体を消すのもここ（#7）。
//!
//! # 確かめられたときだけ書く
//!
//! 判定の規則は [`escrow_domain::liveness`]。ここが足すのは、**その規則が台帳の
//! 行数に出る**こと — 在ることを確かめた回だけ事象が増え、確かめられなかった回は
//! 何も残らない。
//!
//! # どの項目をいつ確かめるかは持たない
//!
//! ここに在るのは「1件を1回確かめる」だけ。順番と時刻はスケジューラが決め、
//! [`Custody::check`] の中の呼び出しがその中で待つ。頻度は巡回の側（#7）。

use std::path::{Path, PathBuf};

use escrow_domain::asset;
use escrow_domain::item::ItemId;
use escrow_domain::liveness::Presence;
use escrow_domain::state::{Event, State, StateName};
use escrow_domain::timestamp::Timestamp;
use escrow_ledger::{Ledger, LedgerError, Projected, Seq};
use escrow_scheduler::{AdapterError, Probe};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CustodyError {
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error("項目 {0} が無い")]
    NoSuchItem(ItemId),
    #[error("項目 {id} は預かり中ではない: {state}")]
    NotHolding { id: ItemId, state: StateName },
    #[error("手元の実体を扱えない: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub struct Custody<'a> {
    ledger: &'a Ledger,
    media_dir: &'a Path,
}

impl<'a> Custody<'a> {
    pub const fn new(ledger: &'a Ledger, media_dir: &'a Path) -> Self {
        Self { ledger, media_dir }
    }

    /// `holding` の1件を配信元と突き合わせ、判定がついたら次の状態まで進める。
    ///
    /// `probe` が空なのは、その種別を確かめる手段を #5 がまだ決めていないとき
    /// （X 投稿）。**観測できなかったのと同じ道を通る**ので、確かめる手段の有無が
    /// 判定の形を変えない。
    ///
    /// # Errors
    ///
    /// 「消えた」と断定できない失敗は、種類を問わずそのまま返す。台帳の側は #5 の
    /// 判定保留のまま — 何も書かず `holding` に残る — で、**返すのは呼ぶ側が失敗の
    /// 種類で動けるようにするため**。cookie の失効はプラットフォーム全体を止め、
    /// 出力を読めないのは仕様変更の疑いになる。
    pub async fn check(
        &self,
        id: ItemId,
        probe: Option<&dyn Probe>,
        now: Timestamp,
    ) -> Result<State, CustodyError> {
        let current = self.load(id).await?;
        let State::Holding { until } = current.item.state else {
            return Err(CustodyError::NotHolding {
                id,
                state: current.item.state.name(),
            });
        };

        let observed = match probe {
            None => Presence::Unknown,
            Some(probe) => match probe.probe(&current.item.url).await {
                Ok(observed) => observed,
                Err(failure) if failure.presence() == Presence::Gone => Presence::Gone,
                Err(failure) => return Err(failure.into()),
            },
        };

        let event = match observed.confirmed() {
            // 証を要求するので、期限が来たというだけで捨てる道は無い（#1）。
            Some(witness) if until <= now => Event::HeldToDeadline(witness),
            Some(witness) => Event::PresenceConfirmed(witness),
            None if observed == Presence::Gone => Event::SourceGone,
            None => return Ok(State::Holding { until }),
        };

        let after = self.append(current, &event, now).await?;

        // **DB を先に更新し、ファイルは後で消す**（#7）。ここで落ちても残るのは
        // 実体だけで、[`Custody::remove_orphans`] が次の回で拾う。
        if after.item.state.is_terminal() {
            self.remove_media(id, Some(after.seq)).await?;
        }

        Ok(after.item.state)
    }

    /// 終わった項目の実体を消し、消したものを返す。
    ///
    /// escrow はメディアを永続化しないので、終端の項目に対応するディレクトリは
    /// 残らない（#1）。**残っているのは、途中で落ちたか、取得しきれなかったぶん**。
    /// #7 は終端の4つを区別せずここへ載せている — 取り切れなかった断片も追跡する
    /// 相手がいないので、残せば `min_free_gib` を静かに削る。
    ///
    /// **置き場所の側から回る。** 台帳を先に読む形だと、配信元ごと消えて行が
    /// 残っていない項目の置き場所が、誰にも見つからない（#1 の削除の連鎖）。
    ///
    /// # Errors
    ///
    /// 台帳を読めないか、ディレクトリを扱えないとき。
    pub async fn remove_orphans(&self) -> Result<Vec<ItemId>, CustodyError> {
        // 消す前に落ちて、退避したまま残っているもの。
        for (id, staged) in asset::staged(self.media_dir).map_err(|source| self.io(source))? {
            if self.finished(id).await? {
                asset::remove_staged(&staged).map_err(|source| self.io(source))?;
            }
        }

        let mut removed = Vec::new();
        for id in asset::item_ids(self.media_dir).map_err(|source| self.io(source))? {
            let projected = self.ledger.item(id).await?;
            let finished = projected
                .as_ref()
                .is_none_or(|projected| projected.item.state.is_terminal());

            if finished && self.remove_media(id, projected.map(|p| p.seq)).await? {
                removed.push(id);
            }
        }

        Ok(removed)
    }

    /// 実体を消す。**退避してから、台帳が動いていないことを確かめて消す。**
    ///
    /// 終端の項目も、人が再取得を指示すれば `waiting` から始まる（#1）。読んだ姿の
    /// まま消しに行くと、その間に始まった取得のものまで消せてしまう。番号が動いて
    /// いなければ再取得は1度も起きていないので、退避したのは終わった項目のもの。
    ///
    /// 動いていたら消さずに置く。誤って消すと戻らないが、置けば次の回で片付く
    /// （#5 の「間違える向きを片側へ寄せる」と同じ向き）。
    async fn remove_media(&self, id: ItemId, anchor: Option<Seq>) -> Result<bool, CustodyError> {
        let Some(staged) =
            asset::stage_for_removal(self.media_dir, id).map_err(|source| self.io(source))?
        else {
            return Ok(false);
        };

        if self.ledger.item(id).await?.map(|p| p.seq) != anchor {
            return Ok(false);
        }

        asset::remove_staged(&staged).map_err(|source| self.io(source))?;
        Ok(true)
    }

    /// もう手元に実体を置かない項目か。**行が無いのも終わりの1つ** — 配信元ごと
    /// 消えた項目（#1 の削除の連鎖）。
    async fn finished(&self, id: ItemId) -> Result<bool, CustodyError> {
        Ok(self
            .ledger
            .item(id)
            .await?
            .is_none_or(|projected| projected.item.state.is_terminal()))
    }

    fn io(&self, source: std::io::Error) -> CustodyError {
        CustodyError::Io {
            path: self.media_dir.to_path_buf(),
            source,
        }
    }

    async fn load(&self, id: ItemId) -> Result<Projected, CustodyError> {
        self.ledger
            .item(id)
            .await?
            .ok_or(CustodyError::NoSuchItem(id))
    }

    /// 事象を1つ追記し、書けた状態を返す。
    ///
    /// 読んだときの `seq` をそのまま渡すので、途中で誰かが動かしていれば
    /// [`LedgerError::Superseded`] で落ちる（#15）。
    async fn append(
        &self,
        current: Projected,
        event: &Event,
        at: Timestamp,
    ) -> Result<Projected, CustodyError> {
        let id = current.item.id;
        self.ledger.append(id, current.seq, event, at).await?;
        self.load(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_domain::content::{Content, MediaType};
    use escrow_domain::item::Discovered;
    use escrow_domain::source::{Monitoring, SourceId};
    use escrow_domain::state::{Hold, MediaPresence, TranscriptNeed};
    use escrow_domain::url::{self, NormalizedUrl};
    use escrow_ledger::{NewSource, Seq};
    use escrow_scheduler::BoxFuture;
    use std::num::NonZeroU32;

    /// 決まった観測を返すだけの生存確認。スケジューラが見せている trait だけを満たす。
    struct FakeProbe(Result<Presence, AdapterError>);

    impl Probe for FakeProbe {
        fn probe<'a>(
            &'a self,
            _url: &'a NormalizedUrl,
        ) -> BoxFuture<'a, Result<Presence, AdapterError>> {
            Box::pin(async move {
                match &self.0 {
                    Ok(presence) => Ok(*presence),
                    Err(AdapterError::Unavailable { url }) => {
                        Err(AdapterError::Unavailable { url: url.clone() })
                    }
                    Err(AdapterError::Unauthenticated { detail }) => {
                        Err(AdapterError::Unauthenticated {
                            detail: detail.clone(),
                        })
                    }
                    Err(other) => panic!("このテストは {other} を使わない"),
                }
            })
        }
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    fn deadline() -> Timestamp {
        at("2026-03-09T00:30:00+09:00")
    }

    /// 持ち主と配信元を1つ用意する。
    async fn seeded(ledger: &Ledger) -> SourceId {
        let person = ledger.add_person("○○").await.unwrap();
        ledger
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled: true,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days: Some(NonZeroU32::new(7).unwrap()),
                priority: NonZeroU32::MIN,
                monitoring: Monitoring::Continuous,
            })
            .await
            .unwrap()
    }

    /// 起票しただけの項目と、その実体を用意する。
    async fn live_item(
        ledger: &Ledger,
        media_dir: &Path,
        source: SourceId,
        video_id: &str,
    ) -> ItemId {
        let id = ledger
            .discover(
                &Discovered {
                    source_id: source,
                    url: url::normalize_item(&format!(
                        "https://www.youtube.com/watch?v={video_id}"
                    ))
                    .unwrap()
                    .0,
                    published_at: at("2026-03-01T20:00:00+09:00"),
                    scheduled_start_at: None,
                    content: Content::Media {
                        media_type: MediaType::YoutubeVideo,
                        title: "○○の雑談配信".to_owned(),
                    },
                    media: MediaPresence::Present,
                },
                at("2026-03-01T20:00:00+09:00"),
            )
            .await
            .unwrap();

        let dir = asset::item_dir(media_dir, id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("video.1.mp4"), b"x").unwrap();
        id
    }

    /// 取得まで済ませ、期限を伴う `holding` の項目にする。
    ///
    /// 事象は台帳へ直接書く。取得のスライスを呼ばないのは、**スライス同士が互いを
    /// 知らない**ことをテストの側でも守るため（#15）。
    async fn holding_item(
        ledger: &Ledger,
        media_dir: &Path,
        source: SourceId,
        video_id: &str,
    ) -> ItemId {
        let id = live_item(ledger, media_dir, source, video_id).await;

        let seq = ledger
            .append(
                id,
                Seq::FIRST,
                &Event::AcquisitionStarted,
                at("2026-03-01T20:10:00+09:00"),
            )
            .await
            .unwrap();
        ledger
            .append(
                id,
                seq,
                &Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: Hold::Until(deadline()),
                },
                at("2026-03-02T00:30:00+09:00"),
            )
            .await
            .unwrap();

        id
    }

    /// 台帳に積まれた事象の本数。誕生を除く。
    async fn recorded(ledger: &Ledger, id: ItemId) -> usize {
        ledger.log(id).await.unwrap().unwrap().rest.len()
    }

    fn media_exists(media_dir: &Path, id: ItemId) -> bool {
        asset::item_dir(media_dir, id).exists()
    }

    /// 期限まで在った → `discarded`。手元の実体も消える（#1）。
    #[tokio::test]
    async fn present_at_the_deadline_is_discarded() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;

        let state = Custody::new(&ledger, media.path())
            .check(id, Some(&FakeProbe(Ok(Presence::Present))), deadline())
            .await
            .unwrap();

        assert_eq!(state, State::Discarded);
        assert!(!media_exists(media.path(), id), "実体は残らない");
    }

    /// 消えた → `kept`。手元のものは残る（#1）。
    #[tokio::test]
    async fn a_source_that_is_gone_leaves_the_copy_kept() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;

        let state = Custody::new(&ledger, media.path())
            .check(
                id,
                Some(&FakeProbe(Ok(Presence::Gone))),
                at("2026-03-03T00:00:00+09:00"),
            )
            .await
            .unwrap();

        assert_eq!(state, State::Kept);
        assert!(media_exists(media.path(), id), "引き渡しを待つので残る");
    }

    /// 「消えた」と断定できる失敗も、消えたとして扱う（#5）。
    #[tokio::test]
    async fn a_failure_that_proves_absence_counts_as_gone() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;

        let state = Custody::new(&ledger, media.path())
            .check(
                id,
                Some(&FakeProbe(Err(AdapterError::Unavailable {
                    url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_owned(),
                }))),
                at("2026-03-03T00:00:00+09:00"),
            )
            .await
            .unwrap();

        assert_eq!(state, State::Kept);
    }

    /// 期限前に在ることを確かめたら、`presence_confirmed` を1つ書いて `holding` のまま。
    #[tokio::test]
    async fn a_confirmation_before_the_deadline_only_records_the_fact() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;
        let before = recorded(&ledger, id).await;

        let state = Custody::new(&ledger, media.path())
            .check(
                id,
                Some(&FakeProbe(Ok(Presence::Present))),
                at("2026-03-03T00:00:00+09:00"),
            )
            .await
            .unwrap();

        assert_eq!(state, State::Holding { until: deadline() });
        assert_eq!(recorded(&ledger, id).await, before + 1);
    }

    /// 確かめられなければ、期限を過ぎていても捨てない。**行も増えない**（#5）。
    #[tokio::test]
    async fn an_unconfirmed_item_survives_its_deadline() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;
        let before = recorded(&ledger, id).await;
        let long_past = at("2026-12-31T00:00:00+09:00");

        let state = Custody::new(&ledger, media.path())
            .check(id, Some(&FakeProbe(Ok(Presence::Unknown))), long_past)
            .await
            .unwrap();

        assert_eq!(state, State::Holding { until: deadline() });
        assert_eq!(recorded(&ledger, id).await, before, "沈黙は残らない");
        assert!(media_exists(media.path(), id));
    }

    /// 確かめる手段を持たない種別（#5 の X 投稿）も、同じ道を通る。
    #[tokio::test]
    async fn a_type_with_no_way_to_check_takes_the_same_path() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;
        let before = recorded(&ledger, id).await;
        let long_past = at("2026-12-31T00:00:00+09:00");

        let state = Custody::new(&ledger, media.path())
            .check(id, None, long_past)
            .await
            .unwrap();

        assert_eq!(state, State::Holding { until: deadline() });
        assert_eq!(recorded(&ledger, id).await, before);
    }

    /// cookie の失効は項目の問題ではないので、握りつぶさず返す（#5）。
    #[tokio::test]
    async fn an_expired_cookie_reaches_the_caller() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;
        let before = recorded(&ledger, id).await;

        let result = Custody::new(&ledger, media.path())
            .check(
                id,
                Some(&FakeProbe(Err(AdapterError::Unauthenticated {
                    detail: "失効".to_owned(),
                }))),
                deadline(),
            )
            .await;

        assert!(matches!(
            result,
            Err(CustodyError::Adapter(AdapterError::Unauthenticated { .. }))
        ));
        assert_eq!(recorded(&ledger, id).await, before);
    }

    /// 預かり中でないものは受け取らない。
    #[tokio::test]
    async fn only_a_held_item_is_checked() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = live_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;

        let result = Custody::new(&ledger, media.path())
            .check(id, Some(&FakeProbe(Ok(Presence::Present))), deadline())
            .await;

        assert!(matches!(
            result,
            Err(CustodyError::NotHolding {
                state: StateName::Waiting,
                ..
            })
        ));
    }

    /// **退避の前後で台帳が動いていたら、消さずに置く。**
    ///
    /// 再取得は終端からしか始まらないので、番号が動いていれば退避したものが
    /// 次の取得のものかもしれない（#1）。誤って消すと戻らない。
    #[tokio::test]
    async fn media_is_not_removed_when_the_ledger_moved_under_it() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;
        let id = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;

        // 読んだ時点の姿と違う番号を渡す = 退避のあいだに誰かが動かしたのと同じ。
        let stale = Some(Seq::FIRST);
        assert_ne!(ledger.item(id).await.unwrap().unwrap().seq, Seq::FIRST);

        let removed = Custody::new(&ledger, media.path())
            .remove_media(id, stale)
            .await
            .unwrap();

        assert!(!removed);
        assert!(!media_exists(media.path(), id), "退避は済んでいる");
        assert_eq!(
            asset::staged(media.path()).unwrap().len(),
            1,
            "消さずに置く"
        );
    }

    /// 行が無い置き場所も消える。配信元ごと消えた項目（#1 の削除の連鎖）。
    ///
    /// **台帳を先に読む形だと、これは誰にも見つからない。**
    #[tokio::test]
    async fn a_place_whose_row_is_gone_is_removed() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let gone = ItemId::new(999);
        let dir = asset::item_dir(media.path(), gone);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("video.1.mp4"), b"x").unwrap();

        let removed = Custody::new(&ledger, media.path())
            .remove_orphans()
            .await
            .unwrap();

        assert_eq!(removed, vec![gone]);
        assert!(!dir.exists());
    }

    /// 退避したまま残っているものは、その項目が終わっていれば片付く。
    #[tokio::test]
    async fn leftover_staging_is_cleared_only_for_finished_items() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&ledger).await;

        let live = live_item(&ledger, media.path(), source, "9B7SFrpmzL0").await;
        let of_live = asset::stage_for_removal(media.path(), live)
            .unwrap()
            .unwrap();

        let finished = live_item(&ledger, media.path(), source, "Ks-_Mh1QhMc").await;
        let seq = ledger.item(finished).await.unwrap().unwrap().seq;
        ledger
            .append(finished, seq, &Event::Deleted, deadline())
            .await
            .unwrap();
        let of_finished = asset::stage_for_removal(media.path(), finished)
            .unwrap()
            .unwrap();

        Custody::new(&ledger, media.path())
            .remove_orphans()
            .await
            .unwrap();

        assert!(of_live.exists(), "進行中のものは残す");
        assert!(!of_finished.exists());
    }

    /// 終端の項目に残った実体を消し、進行中のものには触らない（#7）。
    ///
    /// **取り切れなかったぶんも消す。** #7 は終端の4つを区別していない。
    #[tokio::test]
    async fn orphans_are_removed_and_live_items_are_left_alone() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let custody = Custody::new(&ledger, media.path());

        let source = seeded(&ledger).await;
        let held = holding_item(&ledger, media.path(), source, "dQw4w9WgXcQ").await;
        let live = live_item(&ledger, media.path(), source, "9B7SFrpmzL0").await;

        // 事象だけ書き、実体を消さずに落ちたのと同じ形にする。
        let discarded = holding_item(&ledger, media.path(), source, "zoCXUR4U804").await;
        let seq = ledger.item(discarded).await.unwrap().unwrap().seq;
        ledger
            .append(
                discarded,
                seq,
                &Event::HeldToDeadline(Presence::Present.confirmed().unwrap()),
                deadline(),
            )
            .await
            .unwrap();

        // 取得しきれずに終わったもの。断片が手元に残る。
        let failed = live_item(&ledger, media.path(), source, "Ks-_Mh1QhMc").await;
        let seq = ledger
            .append(
                failed,
                Seq::FIRST,
                &Event::AcquisitionStarted,
                at("2026-03-01T20:10:00+09:00"),
            )
            .await
            .unwrap();
        ledger
            .append(
                failed,
                seq,
                &Event::RetriesExhausted,
                at("2026-03-01T21:00:00+09:00"),
            )
            .await
            .unwrap();

        let removed = custody.remove_orphans().await.unwrap();
        assert_eq!(removed.len(), 2, "{removed:?}");
        assert!(!media_exists(media.path(), discarded));
        assert!(!media_exists(media.path(), failed));
        assert!(media_exists(media.path(), held));
        assert!(media_exists(media.path(), live));
    }
}

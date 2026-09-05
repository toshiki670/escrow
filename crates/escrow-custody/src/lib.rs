//! 預かりの見張り（#15 のスライス）。
//!
//! `holding` の項目を1件受け取り、配信元と突き合わせる。期限まで在り続けたものを
//! 捨て、消えたものは手元に残す（#1）。終端に達した項目の実体を消すのもここ（#7）。
//!
//! # 確かめられたときだけ書く
//!
//! #5 は判定を**分類ではなく非対称性**で決めている。在ることを確かめられたときだけ
//! 事象が増え、確かめられなかった回は行が増えない。**沈黙が記録されない**ことが、
//! そのまま「期限が過ぎていても、直近の確認で『在る』が取れていなければ捨てない」に
//! なる。誤って捨てると手元のメディアは戻らないが、誤って保留すると預かりが延びる
//! だけなので、間違える向きが片側へ寄る。
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
use escrow_ledger::{Ledger, LedgerError, Projected};
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
    /// 「消えた」と断定できない失敗は、項目ではなく escrow 側の問題（#5）。cookie の
    /// 失効はプラットフォーム全体を止め、ツールの出力が読めないのは仕様変更の疑いに
    /// なるので、握りつぶさず呼ぶ側へ返す。
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

        let state = self.append(current, &event, now).await?;

        // **DB を先に更新し、ファイルは後で消す**（#7）。ここで落ちても残るのは
        // 孤児ファイルだけで、[`Custody::remove_orphans`] が次の回で拾う。
        if state.is_terminal() {
            self.remove_media(id)?;
        }

        Ok(state)
    }

    /// 終端に達した項目の実体を消し、消したものを返す。
    ///
    /// escrow はメディアを永続化しないので、終端の項目に対応するディレクトリは
    /// 残らない（#1）。**残っていたら、事象を書いたあとファイルを消す前に落ちた跡**。
    ///
    /// # Errors
    ///
    /// 台帳を読めないか、ディレクトリを消せないとき。
    pub async fn remove_orphans(&self) -> Result<Vec<ItemId>, CustodyError> {
        let mut removed = Vec::new();

        for name in StateName::ALL.into_iter().filter(|name| name.is_terminal()) {
            for projected in self.ledger.items_in_state(name).await? {
                let id = projected.item.id;
                if self.remove_media(id)? {
                    removed.push(id);
                }
            }
        }

        Ok(removed)
    }

    /// 手元のディレクトリを消す。**在ったときだけ真**。
    fn remove_media(&self, id: ItemId) -> Result<bool, CustodyError> {
        let dir = asset::item_dir(self.media_dir, id);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(CustodyError::Io { path: dir, source }),
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
    ) -> Result<State, CustodyError> {
        let id = current.item.id;
        self.ledger.append(id, current.seq, event, at).await?;
        Ok(self.load(id).await?.item.state)
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

    /// 終端の項目に残った実体を消し、進行中のものには触らない（#7）。
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

        assert_eq!(custody.remove_orphans().await.unwrap(), vec![discarded]);
        assert!(!media_exists(media.path(), discarded));
        assert!(media_exists(media.path(), held));
        assert!(media_exists(media.path(), live));
    }
}

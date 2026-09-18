//! 預かり中の項目の生存確認（#15 のスライス）。
//!
//! `holding` の項目を1件受け取り、配信元と突き合わせる。期限まで在り続けたものを
//! 捨て、消えたものは手元に残す（#1）。
//!
//! # 確かめられたときだけ書く
//!
//! 判定の規則は [`escrow_domain::liveness`]。ここが足すのは、**その規則がイベントログの
//! 行数に出る**こと — 在ることを確かめた回だけイベントが増え、確かめられなかった回は
//! 何も残らない。
//!
//! # 持つのは「1件を1回確かめる」だけ
//!
//! ここに在るのは「1件を1回確かめる」だけ。順番と時刻はスケジューラが決め、
//! [`Custody::check`] の中の呼び出しがその中で待つ。頻度は巡回の側（#7）。

use std::path::{Path, PathBuf};

use escrow_domain::asset;
use escrow_domain::item::ItemId;
use escrow_domain::liveness::Presence;
use escrow_domain::state::{Event, State, StateName};
use escrow_domain::timestamp::Timestamp;
use escrow_event_store::{EventStore, EventStoreError, ReadModelRow};
use escrow_scheduler::{AdapterError, Probe};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CustodyError {
    #[error(transparent)]
    EventStore(#[from] EventStoreError),
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
    store: &'a EventStore,
    media_dir: &'a Path,
}

impl<'a> Custody<'a> {
    pub const fn new(store: &'a EventStore, media_dir: &'a Path) -> Self {
        Self { store, media_dir }
    }

    /// `holding` の1件を配信元と突き合わせ、判定がついたら次の状態まで進める。
    ///
    /// `probe` が空なのは、その種別を確かめる手段を #5 がまだ決めていないとき
    /// （X 投稿）。**観測できなかったのと同じ経路を通る**ので、確かめる手段の有無が
    /// 判定の形を変えない。
    ///
    /// # Errors
    ///
    /// 「消えた」と断定できない失敗は、種類を問わずそのまま返す。イベントストアの側は #5 の
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
            // 捨てるには証が要るので、期限が来ただけの回は `holding` に残る（#1）。
            Some(witness) if until <= now => Event::HeldToDeadline(witness),
            Some(witness) => Event::PresenceConfirmed(witness),
            None if observed == Presence::Gone => Event::SourceGone,
            None => return Ok(State::Holding { until }),
        };

        let state = self.append(current, &event, now).await?;

        // 捨てたものは手元から消える（#1 の状態表）。**DB を先に更新し、ファイルは
        // 後で消す** — 逆順にすると、途中で落ちたときに手元に無い `kept` が残る（#7）。
        if state == State::Discarded {
            asset::remove(self.media_dir, id).map_err(|source| CustodyError::Io {
                path: asset::item_dir(self.media_dir, id),
                source,
            })?;
        }

        Ok(state)
    }

    async fn load(&self, id: ItemId) -> Result<ReadModelRow, CustodyError> {
        self.store
            .item(id)
            .await?
            .ok_or(CustodyError::NoSuchItem(id))
    }

    /// イベントを1つ追記し、書けた状態を返す。
    ///
    /// 読んだときの `seq` をそのまま渡すので、途中で誰かが動かしていれば
    /// [`EventStoreError::Superseded`] で落ちる（#15）。
    async fn append(
        &self,
        current: ReadModelRow,
        event: &Event,
        at: Timestamp,
    ) -> Result<State, CustodyError> {
        let id = current.item.id;
        self.store.append(id, current.seq, event, at).await?;
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
    use escrow_event_store::{NewSource, Seq};
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
    async fn seeded(store: &EventStore) -> SourceId {
        let person = store.add_person("○○").await.unwrap();
        store
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
        store: &EventStore,
        media_dir: &Path,
        source: SourceId,
        video_id: &str,
    ) -> ItemId {
        let id = store
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
    /// イベントはイベントストアへ直接書く。取得のスライスを呼ばないのは、**スライス同士が互いを
    /// 知らない**ことをテストの側でも守るため（#15）。
    async fn holding_item(
        store: &EventStore,
        media_dir: &Path,
        source: SourceId,
        video_id: &str,
    ) -> ItemId {
        let id = live_item(store, media_dir, source, video_id).await;

        let seq = store
            .append(
                id,
                Seq::FIRST,
                &Event::AcquisitionStarted,
                at("2026-03-01T20:10:00+09:00"),
            )
            .await
            .unwrap();
        store
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

    /// イベントログに在るイベントの本数。誕生を除く。
    async fn recorded(store: &EventStore, id: ItemId) -> usize {
        store.log(id).await.unwrap().unwrap().rest.len()
    }

    fn media_exists(media_dir: &Path, id: ItemId) -> bool {
        asset::item_dir(media_dir, id).exists()
    }

    /// 期限まで在った → `discarded`。手元の実体も消える（#1）。
    #[tokio::test]
    async fn present_at_the_deadline_is_discarded() {
        let store = EventStore::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&store).await;
        let id = holding_item(&store, media.path(), source, "dQw4w9WgXcQ").await;

        let state = Custody::new(&store, media.path())
            .check(id, Some(&FakeProbe(Ok(Presence::Present))), deadline())
            .await
            .unwrap();

        assert_eq!(state, State::Discarded);
        assert!(!media_exists(media.path(), id), "実体は残らない");
    }

    /// 消えた → `kept`。手元のものは残る（#1）。
    #[tokio::test]
    async fn a_source_that_is_gone_leaves_the_copy_kept() {
        let store = EventStore::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&store).await;
        let id = holding_item(&store, media.path(), source, "dQw4w9WgXcQ").await;

        let state = Custody::new(&store, media.path())
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
        let store = EventStore::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&store).await;
        let id = holding_item(&store, media.path(), source, "dQw4w9WgXcQ").await;

        let state = Custody::new(&store, media.path())
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
        let store = EventStore::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&store).await;
        let id = holding_item(&store, media.path(), source, "dQw4w9WgXcQ").await;
        let before = recorded(&store, id).await;

        let state = Custody::new(&store, media.path())
            .check(
                id,
                Some(&FakeProbe(Ok(Presence::Present))),
                at("2026-03-03T00:00:00+09:00"),
            )
            .await
            .unwrap();

        assert_eq!(state, State::Holding { until: deadline() });
        assert_eq!(recorded(&store, id).await, before + 1);
    }

    /// 確かめられなければ、期限を過ぎていても `holding` に残り、**ログもそのまま**（#5）。
    #[tokio::test]
    async fn an_unconfirmed_item_survives_its_deadline() {
        let store = EventStore::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&store).await;
        let id = holding_item(&store, media.path(), source, "dQw4w9WgXcQ").await;
        let before = recorded(&store, id).await;
        let long_past = at("2026-12-31T00:00:00+09:00");

        let state = Custody::new(&store, media.path())
            .check(id, Some(&FakeProbe(Ok(Presence::Unknown))), long_past)
            .await
            .unwrap();

        assert_eq!(state, State::Holding { until: deadline() });
        assert_eq!(recorded(&store, id).await, before, "沈黙は残らない");
        assert!(media_exists(media.path(), id));
    }

    /// 確かめる手段を持たない種別（#5 の X 投稿）も、同じ経路を通る。
    #[tokio::test]
    async fn a_type_with_no_way_to_check_takes_the_same_path() {
        let store = EventStore::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&store).await;
        let id = holding_item(&store, media.path(), source, "dQw4w9WgXcQ").await;
        let before = recorded(&store, id).await;
        let long_past = at("2026-12-31T00:00:00+09:00");

        let state = Custody::new(&store, media.path())
            .check(id, None, long_past)
            .await
            .unwrap();

        assert_eq!(state, State::Holding { until: deadline() });
        assert_eq!(recorded(&store, id).await, before);
    }

    /// cookie の失効は項目の問題ではないので、捨てずに返す（#5）。
    #[tokio::test]
    async fn an_expired_cookie_reaches_the_caller() {
        let store = EventStore::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&store).await;
        let id = holding_item(&store, media.path(), source, "dQw4w9WgXcQ").await;
        let before = recorded(&store, id).await;

        let result = Custody::new(&store, media.path())
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
        assert_eq!(recorded(&store, id).await, before);
    }

    /// 受け取るのは預かり中のものだけ。
    #[tokio::test]
    async fn only_a_held_item_is_checked() {
        let store = EventStore::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let source = seeded(&store).await;
        let id = live_item(&store, media.path(), source, "dQw4w9WgXcQ").await;

        let result = Custody::new(&store, media.path())
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
}

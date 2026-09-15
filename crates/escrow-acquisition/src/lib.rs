//! 実体を手元へ落とす（#15 のスライス）。
//!
//! `waiting` の項目を1件受け取り、取得して次の状態まで進める。行き先は #1 の
//! 3つのスイッチが決める — 文字起こしする実体があるか、預かりの期限があるか。
//!
//! **文字起こしのスライスを呼ばない。** 取得が終われば状態が `transcribing` に
//! なるので、次に誰が拾うかは状態が決める（#15 の Blackboard）。順序をコードの
//! 呼び出し順で持たないから、スライス同士が互いを知らずに済む。
//!
//! リトライ・空き容量の門・待ち行列は Phase 6（#7）。ここは1件を1回運ぶだけ。

use std::path::Path;

use escrow_domain::asset::{self, Asset, transcript_need};
use escrow_domain::item::ItemId;
use escrow_domain::source::SourceId;
use escrow_domain::state::{Event, HoldTooFar, State};
use escrow_domain::timestamp::Timestamp;
use escrow_event_store::{EventStore, EventStoreError, ReadModelRow};
use escrow_scheduler::{Acquire, AdapterError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcquisitionError {
    #[error(transparent)]
    EventStore(#[from] EventStoreError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error("項目 {0} が無い")]
    NoSuchItem(ItemId),
    #[error("配信元 {0} が無い")]
    NoSuchSource(SourceId),
    #[error(transparent)]
    HoldTooFar(#[from] HoldTooFar),
}

pub struct Acquisition<'a> {
    store: &'a EventStore,
    media_dir: &'a Path,
    acquire: &'a dyn Acquire,
}

impl<'a> Acquisition<'a> {
    pub const fn new(store: &'a EventStore, media_dir: &'a Path, acquire: &'a dyn Acquire) -> Self {
        Self {
            store,
            media_dir,
            acquire,
        }
    }

    /// `waiting` の1件を取得し、次の状態まで進める。
    ///
    /// 預かる日数は配信元から**取得が終わった瞬間に読む**（#1）。呼ぶ側から
    /// 受け取らない。
    ///
    /// 途中の状態は都度書く。落ちたときにどこまで進んだかがリードモデルに残り、#6 の
    /// ダッシュボードが「いま動いているもの」を読める。
    pub async fn run(&self, id: ItemId) -> Result<State, AcquisitionError> {
        let current = self.load(id).await?;
        let source_id = current.item.source_id;
        let current = self.step(current, &Event::AcquisitionStarted).await?;

        let dir = asset::item_dir(self.media_dir, id);
        let assets: Vec<Asset> = self.acquire.acquire(&current.item.url, &dir).await?;

        // 何を落とせたかで、文字起こしが要るかが決まる（#1 のスイッチ表）。
        let transcript = transcript_need(&assets);

        // 期限も日数も、この1つの瞬間から決まる。
        let finished = Timestamp::now();
        let source = self
            .store
            .source(source_id)
            .await?
            .ok_or(AcquisitionError::NoSuchSource(source_id))?;
        let hold = source.hold_from(finished)?;

        let current = self
            .append(current, &Event::Acquired { transcript, hold }, finished)
            .await?;

        Ok(current.item.state)
    }

    async fn load(&self, id: ItemId) -> Result<ReadModelRow, AcquisitionError> {
        self.store
            .item(id)
            .await?
            .ok_or(AcquisitionError::NoSuchItem(id))
    }

    /// イベントを1つ追記し、書けた項目を読み直す。
    ///
    /// 読んだときの `seq` をそのまま渡すので、途中で誰かが動かしていれば
    /// [`EventStoreError::Superseded`] で落ちる（#15）。
    async fn step(
        &self,
        current: ReadModelRow,
        event: &Event,
    ) -> Result<ReadModelRow, AcquisitionError> {
        self.append(current, event, Timestamp::now()).await
    }

    async fn append(
        &self,
        current: ReadModelRow,
        event: &Event,
        at: Timestamp,
    ) -> Result<ReadModelRow, AcquisitionError> {
        let id = current.item.id;
        self.store.append(id, current.seq, event, at).await?;
        self.load(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_domain::asset::AssetKind;
    use escrow_domain::content::{Content, MediaType};
    use escrow_domain::item::Discovered;
    use escrow_domain::source::{Monitoring, SourceId};
    use escrow_domain::state::MediaPresence;
    use escrow_domain::url::{self, NormalizedUrl};
    use escrow_event_store::{NewSource, Seq};
    use escrow_scheduler::BoxFuture;
    use std::num::NonZeroU32;
    use std::sync::Mutex;

    /// 落ちるものを置く代わりの取得。スケジューラが見せている trait だけを満たす。
    ///
    /// **`escrow-external` を名前で知らずに差し替えられる**こと自体が、port が
    /// スケジューラの公開 API だという確認になっている（#15）。
    struct FakeAcquire {
        files: Vec<&'static str>,
        calls: Mutex<usize>,
    }

    impl Acquire for FakeAcquire {
        fn acquire<'a>(
            &'a self,
            _url: &'a NormalizedUrl,
            into: &'a Path,
        ) -> BoxFuture<'a, Result<Vec<Asset>, AdapterError>> {
            Box::pin(async move {
                *self.calls.lock().unwrap() += 1;
                std::fs::create_dir_all(into).unwrap();
                for name in &self.files {
                    std::fs::write(into.join(name), b"x").unwrap();
                }
                Ok(asset::scan_dir(into).unwrap())
            })
        }
    }

    struct Failing;

    impl Acquire for Failing {
        fn acquire<'a>(
            &'a self,
            _url: &'a NormalizedUrl,
            _into: &'a Path,
        ) -> BoxFuture<'a, Result<Vec<Asset>, AdapterError>> {
            Box::pin(async move {
                Err(AdapterError::Transient {
                    program: "yt-dlp".to_owned(),
                    detail: "落ちた".to_owned(),
                })
            })
        }
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    async fn waiting_item(store: &EventStore) -> (SourceId, ItemId) {
        waiting_item_holding_for(store, None).await
    }

    async fn waiting_item_holding_for(
        store: &EventStore,
        hold_days: Option<NonZeroU32>,
    ) -> (SourceId, ItemId) {
        let person = store.add_person("○○").await.unwrap();
        let source = store
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled: true,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days,
                priority: NonZeroU32::MIN,
                monitoring: Monitoring::Continuous,
            })
            .await
            .unwrap();

        let id = store
            .discover(
                &Discovered {
                    source_id: source,
                    url: url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
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
        (source, id)
    }

    fn takes(files: &[&'static str]) -> FakeAcquire {
        FakeAcquire {
            files: files.to_vec(),
            calls: Mutex::new(0),
        }
    }

    /// 音が入っていれば文字起こしへ回る（#1 のスイッチ表）。
    ///
    /// **ここで文字起こしを呼ばない。** 状態が `transcribing` になるだけで、
    /// 次に誰が拾うかは状態が決める（#15）。
    #[tokio::test]
    async fn audible_media_stops_at_transcribing() {
        let store = EventStore::open_in_memory().await.unwrap();
        let (_, id) = waiting_item_holding_for(&store, NonZeroU32::new(7)).await;
        let media = tempfile::tempdir().unwrap();
        let acquire = takes(&["video.1.mp4"]);

        let state = Acquisition::new(&store, media.path(), &acquire)
            .run(id)
            .await
            .unwrap();

        assert_eq!(state.name(), escrow_domain::state::StateName::Transcribing);
        assert!(state.hold_until().is_some(), "期限を伴って文字起こしへ進む");
        assert_eq!(*acquire.calls.lock().unwrap(), 1);
    }

    /// 画像だけなら文字起こしを飛ばす。行き先は期限だけが決める（#1）。
    #[tokio::test]
    async fn images_alone_skip_transcription() {
        for (hold_days, expects_a_deadline) in [(None, false), (NonZeroU32::new(7), true)] {
            let store = EventStore::open_in_memory().await.unwrap();
            let (_, id) = waiting_item_holding_for(&store, hold_days).await;
            let media = tempfile::tempdir().unwrap();

            let state = Acquisition::new(&store, media.path(), &takes(&["image.1.jpg"]))
                .run(id)
                .await
                .unwrap();

            assert_eq!(state.hold_until().is_some(), expects_a_deadline);
            assert_eq!(
                state.name(),
                if expects_a_deadline {
                    escrow_domain::state::StateName::Holding
                } else {
                    escrow_domain::state::StateName::Kept
                }
            );
        }
    }

    /// 落としたものが手元に残ること。
    #[tokio::test]
    async fn what_was_downloaded_stays_on_disk() {
        let store = EventStore::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();

        Acquisition::new(
            &store,
            media.path(),
            &takes(&["video.1.mp4", "video.2.mp4"]),
        )
        .run(id)
        .await
        .unwrap();

        let written = asset::scan(media.path(), id).unwrap();
        assert_eq!(
            written
                .iter()
                .filter(|a| a.kind == AssetKind::Video)
                .count(),
            2
        );
    }

    /// 取得で落ちたら `acquiring` のまま残る。リードモデルを見れば、どこで止まったか分かる。
    ///
    /// リトライと `error` への遷移は Phase 6 の担当（#7）。
    #[tokio::test]
    async fn a_failed_download_leaves_the_item_where_it_stopped() {
        let store = EventStore::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();

        let result = Acquisition::new(&store, media.path(), &Failing)
            .run(id)
            .await;

        assert!(matches!(result, Err(AcquisitionError::Adapter(_))));
        assert_eq!(
            store.item(id).await.unwrap().unwrap().item.state,
            State::Acquiring
        );
    }

    /// 期限の起点は**取得が終わった瞬間**（#1）。
    ///
    /// 取得に時間が掛かるほど、始めた時刻を起点にする形との差が開く。ここでは
    /// 取得の中で時間を進め、期限がそちら側から数えられていることを見る。
    #[tokio::test]
    async fn the_deadline_counts_from_when_the_download_finished() {
        struct Slow;
        impl Acquire for Slow {
            fn acquire<'a>(
                &'a self,
                _url: &'a NormalizedUrl,
                into: &'a Path,
            ) -> BoxFuture<'a, Result<Vec<Asset>, AdapterError>> {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
                    std::fs::create_dir_all(into).unwrap();
                    std::fs::write(into.join("image.1.jpg"), b"x").unwrap();
                    Ok(asset::scan_dir(into).unwrap())
                })
            }
        }

        let store = EventStore::open_in_memory().await.unwrap();
        let (_, id) = waiting_item_holding_for(&store, NonZeroU32::new(7)).await;
        let media = tempfile::tempdir().unwrap();

        let started = Timestamp::now();
        let state = Acquisition::new(&store, media.path(), &Slow)
            .run(id)
            .await
            .unwrap();

        let until = state.hold_until().expect("期限を持つ");
        let from_start = started.plus_days(NonZeroU32::new(7).unwrap()).unwrap();
        assert!(
            until > from_start,
            "始めた時刻から数えている: {until} <= {from_start}"
        );
    }

    /// 図に無い出発点からは動かない。
    #[tokio::test]
    async fn only_a_waiting_item_can_start() {
        let store = EventStore::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();
        store
            .append(id, Seq::FIRST, &Event::Deleted, Timestamp::now())
            .await
            .unwrap();

        let result = Acquisition::new(&store, media.path(), &takes(&["video.1.mp4"]))
            .run(id)
            .await;

        assert!(matches!(
            result,
            Err(AcquisitionError::EventStore(
                EventStoreError::IllegalTransition(_)
            ))
        ));
    }
}

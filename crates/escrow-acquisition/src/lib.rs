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

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use escrow_domain::asset::{self, Asset, transcript_need};
use escrow_domain::item::ItemId;
use escrow_domain::state::{Event, Hold, HoldTooFar, State, TranscriptNeed};
use escrow_domain::timestamp::Timestamp;
use escrow_ledger::{Ledger, LedgerError, Projected};
use escrow_scheduler::{Acquire, AdapterError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcquisitionError {
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error("項目 {0} が無い")]
    NoSuchItem(ItemId),
    #[error(transparent)]
    HoldTooFar(#[from] HoldTooFar),
    #[error("手元の実体を扱えない: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub struct Acquisition<'a, A> {
    ledger: &'a Ledger,
    media_dir: &'a Path,
    acquire: &'a A,
}

impl<'a, A: Acquire> Acquisition<'a, A> {
    pub const fn new(ledger: &'a Ledger, media_dir: &'a Path, acquire: &'a A) -> Self {
        Self {
            ledger,
            media_dir,
            acquire,
        }
    }

    /// `waiting` の1件を取得し、次の状態まで進める。
    ///
    /// `hold_days` は配信元の既定値（#1）。**期限になるのは取得が終わった瞬間**で、
    /// ここで1つの日時に変わったあとは `Item` が持つ。始める前に出しておくと、
    /// 数時間の録画ではその長さぶん期限が早まる。
    ///
    /// 途中の状態は都度書く。落ちたときにどこまで進んだかが台帳に残り、#6 の
    /// ダッシュボードが「いま動いているもの」を読める。
    pub async fn run(
        &self,
        id: ItemId,
        hold_days: Option<NonZeroU32>,
    ) -> Result<State, AcquisitionError> {
        let current = self.load(id).await?;
        let current = self.step(current, &Event::AcquisitionStarted).await?;

        let dir = asset::item_dir(self.media_dir, id);
        let assets: Vec<Asset> = self
            .acquire
            .acquire(&current.item.url, current.item.content_type(), &dir)
            .await?;

        // 何を落とせたかで、文字起こしが要るかが決まる（#1 のスイッチ表）。
        let transcript = transcript_need(&assets);
        let finished = Timestamp::now();
        let hold = Hold::from_days(hold_days, finished)?;
        let current = self
            .append(current, &Event::Acquired { transcript, hold }, finished)
            .await?;

        Ok(current.item.state)
    }

    async fn load(&self, id: ItemId) -> Result<Projected, AcquisitionError> {
        self.ledger
            .item(id)
            .await?
            .ok_or(AcquisitionError::NoSuchItem(id))
    }

    /// 事象を1つ追記し、書けた項目を読み直す。
    ///
    /// 読んだときの `seq` をそのまま渡すので、途中で誰かが動かしていれば
    /// [`LedgerError::Superseded`] で落ちる（#15）。
    async fn step(&self, current: Projected, event: &Event) -> Result<Projected, AcquisitionError> {
        self.append(current, event, Timestamp::now()).await
    }

    async fn append(
        &self,
        current: Projected,
        event: &Event,
        at: Timestamp,
    ) -> Result<Projected, AcquisitionError> {
        let id = current.item.id;
        self.ledger.append(id, current.seq, event, at).await?;
        self.load(id).await
    }
}

/// 取得の結果、文字起こしへ回るか。呼ぶ側が次の拾い手を決めるときに使う。
pub const fn goes_to_transcription(transcript: TranscriptNeed) -> bool {
    matches!(transcript, TranscriptNeed::Needed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_domain::asset::AssetKind;
    use escrow_domain::content::{Content, ContentType, MediaType};
    use escrow_domain::item::Discovered;
    use escrow_domain::source::{Monitoring, SourceId};
    use escrow_domain::state::MediaPresence;
    use escrow_domain::url::{self, NormalizedUrl};
    use escrow_ledger::{NewSource, Seq};
    use std::num::NonZeroU32;
    use std::sync::Mutex;

    /// 落ちるものを置く代わりの取得。スケジューラが見せている口だけを満たす。
    ///
    /// **`escrow-external` を名前で知らずに差し替えられる**こと自体が、port が
    /// スケジューラの公開 API だという確認になっている（#15）。
    struct FakeAcquire {
        files: Vec<&'static str>,
        calls: Mutex<usize>,
    }

    impl Acquire for FakeAcquire {
        async fn acquire(
            &self,
            _url: &NormalizedUrl,
            _content_type: ContentType,
            into: &Path,
        ) -> Result<Vec<Asset>, AdapterError> {
            *self.calls.lock().unwrap() += 1;
            std::fs::create_dir_all(into).unwrap();
            for name in &self.files {
                std::fs::write(into.join(name), b"x").unwrap();
            }
            Ok(asset::scan_dir(into).unwrap())
        }
    }

    struct Failing;

    impl Acquire for Failing {
        async fn acquire(
            &self,
            _url: &NormalizedUrl,
            _content_type: ContentType,
            _into: &Path,
        ) -> Result<Vec<Asset>, AdapterError> {
            Err(AdapterError::Transient {
                program: "yt-dlp".to_owned(),
                detail: "落ちた".to_owned(),
            })
        }
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    async fn waiting_item(ledger: &Ledger) -> (SourceId, ItemId) {
        let person = ledger.add_person("○○").await.unwrap();
        let source = ledger
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled: true,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days: None,
                priority: NonZeroU32::MIN,
                monitoring: Monitoring::Continuous,
            })
            .await
            .unwrap();

        let id = ledger
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
        let ledger = Ledger::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&ledger).await;
        let media = tempfile::tempdir().unwrap();
        let acquire = takes(&["video.1.mp4"]);

        let state = Acquisition::new(&ledger, media.path(), &acquire)
            .run(id, NonZeroU32::new(7))
            .await
            .unwrap();

        assert_eq!(state.name(), escrow_domain::state::StateName::Transcribing);
        assert!(state.hold_until().is_some(), "期限を伴って文字起こしへ進む");
        assert_eq!(*acquire.calls.lock().unwrap(), 1);
        assert!(goes_to_transcription(TranscriptNeed::Needed));
    }

    /// 画像だけなら文字起こしを飛ばす。行き先は期限だけが決める（#1）。
    #[tokio::test]
    async fn images_alone_skip_transcription() {
        for (hold_days, expects_a_deadline) in [(None, false), (NonZeroU32::new(7), true)] {
            let ledger = Ledger::open_in_memory().await.unwrap();
            let (_, id) = waiting_item(&ledger).await;
            let media = tempfile::tempdir().unwrap();

            let state = Acquisition::new(&ledger, media.path(), &takes(&["image.1.jpg"]))
                .run(id, hold_days)
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
        let ledger = Ledger::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&ledger).await;
        let media = tempfile::tempdir().unwrap();

        Acquisition::new(
            &ledger,
            media.path(),
            &takes(&["video.1.mp4", "video.2.mp4"]),
        )
        .run(id, None)
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

    /// 取得で落ちたら `acquiring` のまま残る。台帳を見れば、どこで止まったか分かる。
    ///
    /// リトライと `error` への遷移は Phase 6 の担当（#7）。
    #[tokio::test]
    async fn a_failed_download_leaves_the_item_where_it_stopped() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&ledger).await;
        let media = tempfile::tempdir().unwrap();

        let result = Acquisition::new(&ledger, media.path(), &Failing)
            .run(id, None)
            .await;

        assert!(matches!(result, Err(AcquisitionError::Adapter(_))));
        assert_eq!(
            ledger.item(id).await.unwrap().unwrap().item.state,
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
            async fn acquire(
                &self,
                _url: &NormalizedUrl,
                _content_type: ContentType,
                into: &Path,
            ) -> Result<Vec<Asset>, AdapterError> {
                tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
                std::fs::create_dir_all(into).unwrap();
                std::fs::write(into.join("image.1.jpg"), b"x").unwrap();
                Ok(asset::scan_dir(into).unwrap())
            }
        }

        let ledger = Ledger::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&ledger).await;
        let media = tempfile::tempdir().unwrap();

        let started = Timestamp::now();
        let state = Acquisition::new(&ledger, media.path(), &Slow)
            .run(id, NonZeroU32::new(7))
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
        let ledger = Ledger::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&ledger).await;
        let media = tempfile::tempdir().unwrap();
        ledger
            .append(id, Seq::FIRST, &Event::Deleted, Timestamp::now())
            .await
            .unwrap();

        let result = Acquisition::new(&ledger, media.path(), &takes(&["video.1.mp4"]))
            .run(id, None)
            .await;

        assert!(matches!(
            result,
            Err(AcquisitionError::Ledger(LedgerError::IllegalTransition(_)))
        ));
    }
}

//! 手元の実体を文字起こしする（#15 のスライス）。
//!
//! `transcribing` の項目を1件受け取り、**断片ごとに1本**作って `Transcribed` まで
//! 進める。断片間の空白時間が分からないので、通しのタイムスタンプに繋げられない
//! ため（#1）。
//!
//! **取得のスライスを知らない。** 何を文字起こしするかは手元のディレクトリを走査して
//! 決めるので、前の段から渡してもらう必要が無い。行き先も `Transcribing` が伴っている
//! 期限が決めるので、`Transcribed` は値を運ばない（#1）。

use std::path::{Path, PathBuf};

use escrow_domain::asset::{self, Asset};
use escrow_domain::item::ItemId;
use escrow_domain::state::{Event, State};
use escrow_domain::timestamp::Timestamp;
use escrow_ledger::{Ledger, LedgerError, Projected};
use escrow_scheduler::{AdapterError, Transcribe};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TranscriptionError {
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error("項目 {0} が無い")]
    NoSuchItem(ItemId),
    #[error("手元の実体を扱えない: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub struct Transcription<'a, T> {
    ledger: &'a Ledger,
    media_dir: &'a Path,
    transcribe: &'a T,
}

impl<'a, T: Transcribe> Transcription<'a, T> {
    pub const fn new(ledger: &'a Ledger, media_dir: &'a Path, transcribe: &'a T) -> Self {
        Self {
            ledger,
            media_dir,
            transcribe,
        }
    }

    /// `transcribing` の1件を文字起こしし、次の状態まで進める。
    ///
    /// 行き先は `Transcribing` が伴っている期限が決める — 期限があれば `holding`、
    /// 無ければ `kept`（#1）。
    pub async fn run(&self, id: ItemId) -> Result<State, TranscriptionError> {
        let current = self.load(id).await?;
        let dir = asset::item_dir(self.media_dir, id);

        let assets = asset::scan(self.media_dir, id).map_err(|source| TranscriptionError::Io {
            path: dir.clone(),
            source,
        })?;

        // 断片ごとに1本（#1）。`video.2.mp4` には `transcript.2.vtt` が対応する。
        for source in assets.iter().filter(|a: &&Asset| a.kind.is_audible()) {
            let media = dir.join(source.file_name());
            self.transcribe
                .transcribe(&media, &dir, source.ordinal)
                .await?;
        }

        let current = self.step(current, &Event::Transcribed).await?;
        Ok(current.item.state)
    }

    async fn load(&self, id: ItemId) -> Result<Projected, TranscriptionError> {
        self.ledger
            .item(id)
            .await?
            .ok_or(TranscriptionError::NoSuchItem(id))
    }

    async fn step(
        &self,
        current: Projected,
        event: &Event,
    ) -> Result<Projected, TranscriptionError> {
        let id = current.item.id;
        self.ledger
            .append(id, current.seq, event, Timestamp::now())
            .await?;
        self.load(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_domain::asset::AssetKind;
    use escrow_domain::content::{Content, MediaType};
    use escrow_domain::item::Discovered;
    use escrow_domain::source::Monitoring;
    use escrow_domain::state::{Hold, MediaPresence, TranscriptNeed};
    use escrow_domain::url;
    use escrow_ledger::{NewSource, Seq};
    use std::num::NonZeroU32;
    use std::sync::Mutex;

    /// 呼ばれた実体を控えるだけの文字起こし。
    struct FakeTranscribe {
        calls: Mutex<Vec<String>>,
    }

    impl Transcribe for FakeTranscribe {
        async fn transcribe(
            &self,
            media: &Path,
            into: &Path,
            ordinal: std::num::NonZeroU32,
        ) -> Result<Asset, AdapterError> {
            self.calls
                .lock()
                .unwrap()
                .push(media.file_name().unwrap().to_string_lossy().into_owned());

            let asset = Asset::new(AssetKind::Transcript, ordinal, "vtt");
            std::fs::write(into.join(asset.file_name()), b"WEBVTT\n").unwrap();
            Ok(asset)
        }
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    fn deadline() -> Timestamp {
        at("2026-03-09T00:30:00+09:00")
    }

    /// 取得まで済ませ、`transcribing` の項目とその実体を用意する。
    ///
    /// 事象は台帳へ直接書く。取得のスライスを呼ばないのは、**スライス同士が互いを
    /// 知らない**ことをテストの側でも守るため（#15）。
    async fn transcribing_item(ledger: &Ledger, media_dir: &Path, hold: Hold) -> ItemId {
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
                    transcript: TranscriptNeed::Needed,
                    hold,
                },
                at("2026-03-02T00:30:00+09:00"),
            )
            .await
            .unwrap();

        let dir = asset::item_dir(media_dir, id);
        std::fs::create_dir_all(&dir).unwrap();
        id
    }

    fn put(dir: &Path, names: &[&str]) {
        for name in names {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
    }

    /// 断片ごとに1本（#1）。
    #[tokio::test]
    async fn each_fragment_gets_its_own_transcript() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let id = transcribing_item(&ledger, media.path(), Hold::None).await;
        put(
            &asset::item_dir(media.path(), id),
            &["video.1.mp4", "video.2.mp4", "video.3.mp4"],
        );

        let transcribe = FakeTranscribe {
            calls: Mutex::new(Vec::new()),
        };
        Transcription::new(&ledger, media.path(), &transcribe)
            .run(id)
            .await
            .unwrap();

        assert_eq!(
            transcribe.calls.lock().unwrap().as_slice(),
            ["video.1.mp4", "video.2.mp4", "video.3.mp4"]
        );
        let written = asset::scan(media.path(), id).unwrap();
        assert_eq!(
            written
                .iter()
                .filter(|a| a.kind == AssetKind::Transcript)
                .count(),
            3
        );
    }

    /// 音の入っていないものは飛ばす（#1 のスイッチ表）。
    #[tokio::test]
    async fn images_are_not_transcribed() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let id = transcribing_item(&ledger, media.path(), Hold::None).await;
        put(
            &asset::item_dir(media.path(), id),
            &["image.1.jpg", "image.2.jpg"],
        );

        let transcribe = FakeTranscribe {
            calls: Mutex::new(Vec::new()),
        };
        Transcription::new(&ledger, media.path(), &transcribe)
            .run(id)
            .await
            .unwrap();

        assert!(transcribe.calls.lock().unwrap().is_empty());
    }

    /// 行き先を決めるのは、状態が伴っている期限だけ（#1）。
    ///
    /// 行き先を決めるのは、状態が持っている期限だけ。`Transcribed` は値を運ばない。
    #[tokio::test]
    async fn the_destination_comes_from_the_deadline_the_state_carries() {
        for (hold, expected) in [
            (Hold::None, State::Kept),
            (
                Hold::Until(deadline()),
                State::Holding { until: deadline() },
            ),
        ] {
            let ledger = Ledger::open_in_memory().await.unwrap();
            let media = tempfile::tempdir().unwrap();
            let id = transcribing_item(&ledger, media.path(), hold).await;
            put(&asset::item_dir(media.path(), id), &["video.1.mp4"]);

            let state = Transcription::new(
                &ledger,
                media.path(),
                &FakeTranscribe {
                    calls: Mutex::new(Vec::new()),
                },
            )
            .run(id)
            .await
            .unwrap();

            assert_eq!(state, expected);
        }
    }

    /// 文字起こしで落ちたら `transcribing` のまま残る。
    #[tokio::test]
    async fn a_failed_transcription_leaves_the_item_where_it_stopped() {
        struct Failing;
        impl Transcribe for Failing {
            async fn transcribe(
                &self,
                _media: &Path,
                _into: &Path,
                _ordinal: std::num::NonZeroU32,
            ) -> Result<Asset, AdapterError> {
                Err(AdapterError::Transient {
                    program: "whisper-cli".to_owned(),
                    detail: "落ちた".to_owned(),
                })
            }
        }

        let ledger = Ledger::open_in_memory().await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let id = transcribing_item(&ledger, media.path(), Hold::None).await;
        put(&asset::item_dir(media.path(), id), &["video.1.mp4"]);

        let result = Transcription::new(&ledger, media.path(), &Failing)
            .run(id)
            .await;

        assert!(matches!(result, Err(TranscriptionError::Adapter(_))));
        assert_eq!(
            ledger.item(id).await.unwrap().unwrap().item.state,
            State::Transcribing { hold: Hold::None }
        );
    }
}

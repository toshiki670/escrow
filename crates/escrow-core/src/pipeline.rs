//! 見つけた項目を、引き渡せる状態まで運ぶ。
//!
//! 状態を決めるのは [`crate::state::next`]、書くのは [`Store::apply`] で、ここは
//! **どの事象をいつ起こすか**だけを持つ。外の世界（アダプタ）と台帳（store）を
//! 繋ぐ場所。
//!
//! # 段はそれぞれ独立に呼べる
//!
//! [`Pipeline::acquire`] と [`Pipeline::transcribe`] は別々に呼べる。落ちた項目は
//! その状態のまま台帳に残るので、次の周は**残った状態から**続けられる。
//! 巡回してどれをいつ呼ぶかを決めるのは [`crate::engine`] で、ここは呼ばれた1件を
//! 1段進めるだけ。

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::adapter::{Acquire, AdapterError, Transcribe};
use crate::asset::{self, Asset, AssetKind};
use crate::item::{Item, ItemId};
use crate::state::{Event, HoldPolicy, State, StateName, TranscriptNeed};
use crate::store::{Applied, Store, StoreError};
use crate::timestamp::Timestamp;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    #[error("項目 {0} が無い")]
    NoSuchItem(ItemId),
    #[error("読んだときから状態が動いている。読み直してやり直す")]
    Superseded,
    #[error("項目 {id} は {state} なので、{step}の段には入れない")]
    NotAtThisStep {
        id: ItemId,
        state: StateName,
        step: &'static str,
    },
    #[error("手元の実体を扱えない: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub struct Pipeline<'a, A, T> {
    store: &'a Store,
    media_dir: &'a Path,
    acquire: &'a A,
    transcribe: &'a T,
}

impl<'a, A: Acquire, T: Transcribe> Pipeline<'a, A, T> {
    pub fn new(store: &'a Store, media_dir: &'a Path, acquire: &'a A, transcribe: &'a T) -> Self {
        Self {
            store,
            media_dir,
            acquire,
            transcribe,
        }
    }

    /// `waiting` の項目を、`kept` か `holding` まで運ぶ。
    ///
    /// 途中の状態は都度 DB へ書く。落ちたときにどこまで進んだかが台帳に残り、
    /// #6 のダッシュボードが「いま動いているもの」を読める。
    pub async fn run(&self, id: ItemId, hold: HoldPolicy) -> Result<State, PipelineError> {
        let item = self.load(id).await?;
        let item = self.acquire(&item, hold).await?;

        let item = match item.state {
            State::Transcribing => self.transcribe(&item, hold).await?,
            State::Waiting
            | State::Acquiring
            | State::Holding
            | State::Kept
            | State::Discarded
            | State::Released { .. }
            | State::Deleted
            | State::Error => item,
        };

        Ok(item.state)
    }

    /// 取得の段。`waiting` から始めるか、`acquiring` の続きから。
    ///
    /// 前の周が落ちて `acquiring` のまま残ったものには、事象を重ねない。図に
    /// `acquiring --> acquiring` は無いので、重ねると[`crate::state::next`] が弾く。
    /// 「始める」と「続ける」は台帳の状態で見分けられるので、呼ぶ側は分けなくてよい。
    pub async fn acquire(&self, item: &Item, hold: HoldPolicy) -> Result<Item, PipelineError> {
        let started = match item.state {
            State::Waiting => self.step(item, &Event::AcquisitionStarted).await?,
            State::Acquiring => item.clone(),
            State::Transcribing
            | State::Holding
            | State::Kept
            | State::Discarded
            | State::Released { .. }
            | State::Deleted
            | State::Error => {
                return Err(PipelineError::NotAtThisStep {
                    id: item.id,
                    state: item.state.name(),
                    step: "取得",
                });
            }
        };

        let assets = self.download(&started).await?;
        let transcript = transcript_need(&assets);

        self.step(&started, &Event::Acquired { transcript, hold })
            .await
    }

    /// 文字起こしの段。`transcribing` の項目だけが入れる。
    pub async fn transcribe(&self, item: &Item, hold: HoldPolicy) -> Result<Item, PipelineError> {
        match item.state {
            State::Transcribing => {}
            State::Waiting
            | State::Acquiring
            | State::Holding
            | State::Kept
            | State::Discarded
            | State::Released { .. }
            | State::Deleted
            | State::Error => {
                return Err(PipelineError::NotAtThisStep {
                    id: item.id,
                    state: item.state.name(),
                    step: "文字起こし",
                });
            }
        }

        self.write_transcripts(item).await?;
        self.step(item, &Event::Transcribed { hold }).await
    }

    async fn load(&self, id: ItemId) -> Result<Item, PipelineError> {
        self.store
            .item(id)
            .await?
            .ok_or(PipelineError::NoSuchItem(id))
    }

    /// 事象を1つ適用し、書けた項目を読み直す。
    async fn step(&self, item: &Item, event: &Event) -> Result<Item, PipelineError> {
        match self
            .store
            .apply(item.id, &item.state, event, Timestamp::now())
            .await?
        {
            Applied::Written(_) => self.load(item.id).await,
            Applied::Superseded => Err(PipelineError::Superseded),
        }
    }

    async fn download(&self, item: &Item) -> Result<Vec<Asset>, PipelineError> {
        let dir = asset::item_dir(self.media_dir, item.id);
        Ok(self
            .acquire
            .acquire(&item.url, item.content_type(), &dir)
            .await?)
    }

    /// 断片ごとに1本作る（#1）。`video.2.mp4` には `transcript.2.vtt` が対応する。
    ///
    /// 手元にあるものを数え直すので、前の周が途中で落ちていれば**済んだぶんを
    /// 飛ばして続きから**やる。文字起こしは1本で数分かかるので、やり直しの
    /// たびに全部引き直すと期限までに終わらない。
    async fn write_transcripts(&self, item: &Item) -> Result<(), PipelineError> {
        let dir = asset::item_dir(self.media_dir, item.id);
        let assets = asset::scan(self.media_dir, item.id).map_err(|source| PipelineError::Io {
            path: dir.clone(),
            source,
        })?;

        let done: Vec<_> = assets
            .iter()
            .filter(|a| a.kind == AssetKind::Transcript)
            .map(|a| a.ordinal)
            .collect();

        for source in assets.iter().filter(|a| is_audible(a)) {
            if done.contains(&source.ordinal) {
                continue;
            }
            let media = dir.join(source.file_name());
            self.transcribe
                .transcribe(&media, &dir, source.ordinal)
                .await?;
        }
        Ok(())
    }
}

/// 文字起こしする実体があるか。#1 の3つのスイッチの1つ。
fn transcript_need(assets: &[Asset]) -> TranscriptNeed {
    if assets.iter().any(is_audible) {
        TranscriptNeed::Needed
    } else {
        TranscriptNeed::NotNeeded
    }
}

/// 音が入っているもの。画像だけの投稿は文字起こしできない（#1 のスイッチ表）。
fn is_audible(asset: &Asset) -> bool {
    matches!(asset.kind, AssetKind::Video | AssetKind::Audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Content, ContentType, MediaType};
    use crate::source::SourceId;
    use crate::store::{NewItem, NewSource};
    use crate::url::{self, NormalizedUrl};
    use std::num::NonZeroU32;
    use std::sync::Mutex;

    /// 落ちるものを置く代わりの取得。アダプタの口だけを満たす。
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

    struct FakeTranscribe {
        calls: Mutex<Vec<String>>,
    }

    impl Transcribe for FakeTranscribe {
        async fn transcribe(
            &self,
            media: &Path,
            into: &Path,
            ordinal: NonZeroU32,
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

    async fn waiting_item(store: &Store) -> (SourceId, ItemId) {
        let person = store.add_person("○○").await.unwrap();
        let source = store
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled: true,
                created_at: Timestamp::parse("2026-01-01T00:00:00+09:00").unwrap(),
                hold_days: None,
                discover_interval_minutes: NonZeroU32::new(15).unwrap(),
            })
            .await
            .unwrap();

        let id = store
            .add_item(&NewItem {
                source_id: source,
                url: url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                    .unwrap()
                    .0,
                published_at: Timestamp::parse("2026-03-01T20:00:00+09:00").unwrap(),
                state: State::Waiting,
                state_since: Timestamp::parse("2026-03-01T20:00:00+09:00").unwrap(),
                content: Content::Media {
                    media_type: MediaType::YoutubeVideo,
                    title: "○○の雑談配信".to_owned(),
                },
            })
            .await
            .unwrap();
        (source, id)
    }

    /// #1 の図の経路そのもの。`hold_days` が無いので `holding` を飛ばす。
    #[tokio::test]
    async fn carries_a_waiting_item_to_kept() {
        let store = Store::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();

        let acquire = FakeAcquire {
            files: vec!["video.1.mp4"],
            calls: Mutex::new(0),
        };
        let transcribe = FakeTranscribe {
            calls: Mutex::new(Vec::new()),
        };

        let state = Pipeline::new(&store, media.path(), &acquire, &transcribe)
            .run(id, HoldPolicy::NoHold)
            .await
            .unwrap();

        assert_eq!(state, State::Kept);
        assert_eq!(*acquire.calls.lock().unwrap(), 1);
        assert_eq!(transcribe.calls.lock().unwrap().as_slice(), ["video.1.mp4"]);
        assert_eq!(store.item(id).await.unwrap().unwrap().state, State::Kept);
    }

    /// `hold_days` があれば `holding` を通る（#1）。
    #[tokio::test]
    async fn a_holding_source_stops_at_holding() {
        let store = Store::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();

        let state = Pipeline::new(
            &store,
            media.path(),
            &FakeAcquire {
                files: vec!["video.1.mp4"],
                calls: Mutex::new(0),
            },
            &FakeTranscribe {
                calls: Mutex::new(Vec::new()),
            },
        )
        .run(id, HoldPolicy::Hold)
        .await
        .unwrap();

        assert_eq!(state, State::Holding);
    }

    /// 画像だけなら文字起こしを飛ばして `kept` へ（#1 のスイッチ表）。
    #[tokio::test]
    async fn images_alone_skip_transcription() {
        let store = Store::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();

        let transcribe = FakeTranscribe {
            calls: Mutex::new(Vec::new()),
        };
        let state = Pipeline::new(
            &store,
            media.path(),
            &FakeAcquire {
                files: vec!["image.1.jpg", "image.2.jpg"],
                calls: Mutex::new(0),
            },
            &transcribe,
        )
        .run(id, HoldPolicy::NoHold)
        .await
        .unwrap();

        assert_eq!(state, State::Kept);
        assert!(transcribe.calls.lock().unwrap().is_empty());
    }

    /// 断片ごとに1本（#1）。
    #[tokio::test]
    async fn each_fragment_gets_its_own_transcript() {
        let store = Store::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();

        let transcribe = FakeTranscribe {
            calls: Mutex::new(Vec::new()),
        };
        Pipeline::new(
            &store,
            media.path(),
            &FakeAcquire {
                files: vec!["video.1.mp4", "video.2.mp4", "video.3.mp4"],
                calls: Mutex::new(0),
            },
            &transcribe,
        )
        .run(id, HoldPolicy::NoHold)
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

    /// 取得で落ちたら `acquiring` のまま残る。台帳を見れば、どこで止まったか分かる。
    ///
    /// リトライと `error` への遷移は Phase 4 の担当（#7）。
    #[tokio::test]
    async fn a_failed_download_leaves_the_item_where_it_stopped() {
        let store = Store::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();

        let result = Pipeline::new(
            &store,
            media.path(),
            &Failing,
            &FakeTranscribe {
                calls: Mutex::new(Vec::new()),
            },
        )
        .run(id, HoldPolicy::NoHold)
        .await;

        assert!(matches!(result, Err(PipelineError::Adapter(_))));
        assert_eq!(
            store.item(id).await.unwrap().unwrap().state,
            State::Acquiring
        );
    }

    /// 図に無い出発点からは動かない。台帳を読んだ時点で断るので、
    /// 外部ツールは起動しない。
    #[tokio::test]
    async fn only_a_waiting_item_can_start() {
        let store = Store::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();
        store
            .apply(id, &State::Waiting, &Event::Deleted, Timestamp::now())
            .await
            .unwrap();

        let result = Pipeline::new(
            &store,
            media.path(),
            &FakeAcquire {
                files: vec!["video.1.mp4"],
                calls: Mutex::new(0),
            },
            &FakeTranscribe {
                calls: Mutex::new(Vec::new()),
            },
        )
        .run(id, HoldPolicy::NoHold)
        .await;

        assert!(matches!(
            result,
            Err(PipelineError::NotAtThisStep {
                state: StateName::Deleted,
                ..
            })
        ));
    }

    /// 落ちて `acquiring` のまま残ったものは、続きから運べる。
    /// 事象を重ねないので、図に無い `acquiring --> acquiring` を通らない。
    #[tokio::test]
    async fn a_stalled_acquisition_resumes_where_it_stopped() {
        let store = Store::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();
        let transcribe = FakeTranscribe {
            calls: Mutex::new(Vec::new()),
        };

        // 1周目は取得で落ちる。acquiring のまま残る。
        assert!(
            Pipeline::new(&store, media.path(), &Failing, &transcribe)
                .run(id, HoldPolicy::NoHold)
                .await
                .is_err()
        );
        let stalled = store.item(id).await.unwrap().unwrap();
        assert_eq!(stalled.state, State::Acquiring);

        // 2周目は同じ項目を acquiring から続ける。
        let acquire = FakeAcquire {
            files: vec!["video.1.mp4"],
            calls: Mutex::new(0),
        };
        let state = Pipeline::new(&store, media.path(), &acquire, &transcribe)
            .run(id, HoldPolicy::NoHold)
            .await
            .unwrap();

        assert_eq!(state, State::Kept);
    }

    /// 済んだ文字起こしは引き直さない。1本で数分かかるので、やり直しのたびに
    /// 全部引くと期限までに終わらない。
    #[tokio::test]
    async fn transcription_picks_up_where_it_left_off() {
        let store = Store::open_in_memory().await.unwrap();
        let (_, id) = waiting_item(&store).await;
        let media = tempfile::tempdir().unwrap();
        let dir = asset::item_dir(media.path(), id);

        // video.1 のぶんは前の周で済んでいる、という体。
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("transcript.1.vtt"), b"WEBVTT\n").unwrap();

        let transcribe = FakeTranscribe {
            calls: Mutex::new(Vec::new()),
        };
        Pipeline::new(
            &store,
            media.path(),
            &FakeAcquire {
                files: vec!["video.1.mp4", "video.2.mp4"],
                calls: Mutex::new(0),
            },
            &transcribe,
        )
        .run(id, HoldPolicy::NoHold)
        .await
        .unwrap();

        assert_eq!(
            transcribe.calls.lock().unwrap().as_slice(),
            ["video.2.mp4"],
            "済んでいる 1 を飛ばして 2 だけ引く"
        );
    }
}

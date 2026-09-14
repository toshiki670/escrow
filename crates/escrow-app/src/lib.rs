//! 入口が呼ぶ関数（#82）。
//!
//! 設定を読んで台帳を開く手順と、そこから読む・書く関数をここに置く。入口
//! （`escrow-cli` / `escrow-gui`、#79 の SwiftUI）はこの crate だけを見て、出力の形
//! （`println!`・画面）だけを持つ。画面ごとに足す関数もここに置くので、入口が増えても
//! 書く場所は1つ。
//!
//! 入口が使う型は `pub use` でここから見せる。入口が名前で知る crate はこれ1つ
//! （`tests/dependency_direction.rs`）。
//!
//! 引数は text と `i64` で受ける。**型で締めていない**のは、#79 の UniFFI が運ぶ形を
//! そのまま保つため。

use std::num::NonZeroU32;
use std::path::PathBuf;

use thiserror::Error;

use escrow_acquisition::{Acquisition, AcquisitionError};
use escrow_config::{Config, ConfigError, Dirs, Paths, Resolver};
use escrow_domain::content::{ContentType, UnknownContentType};
use escrow_domain::item::Discovered;
use escrow_domain::source::{Monitoring, MonitoringError};
use escrow_domain::state::{ReleaseReference, StateName, UnknownState};
use escrow_domain::timestamp::{Timestamp, TimestampError};
use escrow_domain::url::{self, TypeHint, UrlError};
use escrow_handover::{Handover, HandoverError};
use escrow_ledger::{Ledger, LedgerError, NewSource};
use escrow_scheduler::{AdapterError, Demand, MissingTool, Scheduler};
use escrow_transcription::{Transcription, TranscriptionError};

#[cfg(any(test, feature = "fixture"))]
mod fixture;
mod listing;

pub use escrow_config::{Resolution, Tool};
pub use escrow_domain::item::ItemId;
pub use escrow_domain::source::{Person, PersonId, SourceId};
pub use escrow_domain::state::State;
pub use escrow_handover::Handed;
pub use listing::Listed;

/// 入口へ返す失敗。
///
/// 台帳・設定・スライスの失敗はそのまま通し、ここで決められなかったものにだけ
/// 名前を付ける。
#[derive(Debug, Error)]
pub enum AppError {
    #[error("設定の置き場所を決められない")]
    Dirs(#[source] ConfigError),
    #[error("設定を読めない")]
    Config(#[source] ConfigError),
    #[error("DB を開けない: {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: LedgerError,
    },
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Handover(#[from] HandoverError),
    #[error(transparent)]
    Acquisition(#[from] AcquisitionError),
    #[error(transparent)]
    Transcription(#[from] TranscriptionError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    /// 要るツールが無い。どこを探したかは [`App::doctor`] が出す。
    #[error(transparent)]
    MissingTool(#[from] MissingTool),
    #[error(transparent)]
    Url(#[from] UrlError),
    #[error(transparent)]
    Monitoring(#[from] MonitoringError),
    #[error(transparent)]
    Timestamp(#[from] TimestampError),
    #[error(transparent)]
    ContentType(#[from] UnknownContentType),
    #[error("知らない状態")]
    State(#[from] UnknownState),
    #[error("重みは1以上")]
    Priority,
    #[error("預かる日数は1日以上")]
    HoldDays,
    #[error("項目 {0} が無い")]
    NoSuchItem(ItemId),
    /// URL が種別を語らない（`/watch?v=` など）。人が種別を添えて呼び直す（#5）。
    #[error("この URL からは種別を決められない")]
    UndecidableType,
}

/// 設定を読み、台帳を開いた状態。入口はこれを1つ持ち、ここの関数だけを呼ぶ。
#[derive(Debug)]
pub struct App {
    config: Config,
    paths: Paths,
    ledger: Ledger,
    resolver: Resolver,
}

/// `doctor` が答えるもの — 外部ツールと文字起こしモデルがどこで見つかるか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    /// ツールごとに、どこで見つかったか。
    pub tools: Vec<(Tool, Resolution)>,
    /// 設定が指す文字起こしモデルの場所。
    ///
    /// [`Resolution::Found`] も場所を持つが、無いときにどこを見たかを言えるのはこちらだけ。
    pub transcribe_model_path: PathBuf,
    /// その場所にモデルが在るか。
    pub transcribe_model: Resolution,
    /// 探した場所。
    pub directories: Vec<PathBuf>,
}

impl App {
    /// 設定の言う場所で台帳を開く。
    ///
    /// # Errors
    ///
    /// 設定の置き場所を決められない・設定を読めない・DB を開けない、のどれか。
    pub async fn open() -> Result<Self, AppError> {
        let dirs = Dirs::discover().map_err(AppError::Dirs)?;
        let config = Config::load(&dirs.config_file()).map_err(AppError::Config)?;
        let paths = Paths::resolve(&config, &dirs);
        let resolver = Resolver::from_env(&config.extra_paths(&dirs));

        let ledger = Ledger::open(&paths.db)
            .await
            .map_err(|source| AppError::Open {
                path: paths.db.clone(),
                source,
            })?;

        Ok(Self {
            config,
            paths,
            ledger,
            resolver,
        })
    }

    /// 外部アクセスの受付。外へ出る呼び出しはすべてここを通る（#13）。
    fn scheduler(&self) -> Result<Scheduler, AppError> {
        Ok(Scheduler::new(&self.config, &self.paths, &self.resolver)?)
    }

    fn handover(&self) -> Handover<'_> {
        Handover::new(&self.ledger, &self.paths.media_dir)
    }

    /// 配信元の持ち主を全部。
    ///
    /// # Errors
    ///
    /// 台帳を読めないとき。
    pub async fn persons(&self) -> Result<Vec<Person>, AppError> {
        Ok(self.ledger.persons().await?)
    }

    /// 選んだ持ち主の項目を、一覧の1行の形で新しい順に。
    ///
    /// 1件ごとに手元の実体を数えるので、行の数だけディレクトリを見に行く。#4 の
    /// `list` と同じ経路で、**画面に出る値と外部が受け取る値をずらさない**ための代償。
    ///
    /// # Errors
    ///
    /// 台帳を読めない・手元の実体を扱えない、のどちらか。
    pub async fn items_of(&self, person: PersonId) -> Result<Vec<Listed>, AppError> {
        let projected = self.ledger.items_of_person(person).await?;
        let handover = self.handover();

        let mut listed = projected
            .iter()
            .map(|projected| {
                handover
                    .handed(&projected.item)
                    .map(|handed| Listed::new(projected.item.published_at, &handed))
            })
            .collect::<Result<Vec<_>, _>>()?;

        listing::newest_first(&mut listed);
        Ok(listed)
    }

    /// #4 の `list`。状態か id で絞り、外部が受け取る形で返す。
    ///
    /// # Errors
    ///
    /// 知らない状態・台帳を読めない・手元の実体を扱えない、のどれか。
    pub async fn list(
        &self,
        state: Option<&str>,
        id: Option<i64>,
    ) -> Result<Vec<Handed>, AppError> {
        let projected = match (state, id) {
            (_, Some(id)) => self
                .ledger
                .item(ItemId::new(id))
                .await?
                .into_iter()
                .collect::<Vec<_>>(),
            (Some(name), None) => {
                let name: StateName = name.parse()?;
                self.ledger.items_in_state(name).await?
            }
            // #4 は状態を絞らない呼び方も許す。既定は引き渡し待ち。
            (None, None) => self.ledger.items_in_state(StateName::Kept).await?,
        };

        let handover = self.handover();
        Ok(projected
            .iter()
            .map(|p| handover.handed(&p.item))
            .collect::<Result<_, _>>()?)
    }

    /// #4 の `release`。消す前の姿を返す。
    ///
    /// # Errors
    ///
    /// 項目が無い・引き渡せる状態でない・台帳や実体を触れない、のどれか。
    pub async fn release(&self, id: i64, reference: Option<&str>) -> Result<Handed, AppError> {
        Ok(self
            .handover()
            .release(ItemId::new(id), reference.map(ReleaseReference::new))
            .await?)
    }

    /// 配信元の持ち主を登録する。
    ///
    /// # Errors
    ///
    /// 台帳へ書けないとき。
    pub async fn add_person(&self, name: &str) -> Result<PersonId, AppError> {
        Ok(self.ledger.add_person(name).await?)
    }

    /// 監視対象を登録する。日時は ISO 8601 の text で受け、両方か両方無しで対にする（#1）。
    ///
    /// # Errors
    ///
    /// URL が配信元の形でない・重みや日数が 0・監視期間が対でない・台帳へ書けない、
    /// のどれか。
    pub async fn add_source(
        &self,
        person: i64,
        raw_url: &str,
        priority: u32,
        monitor_from: Option<&str>,
        monitor_until: Option<&str>,
        hold_days: Option<u32>,
    ) -> Result<SourceId, AppError> {
        let url = url::normalize_source(raw_url)?;
        let priority = NonZeroU32::new(priority).ok_or(AppError::Priority)?;
        let hold_days = hold_days
            .map(|d| NonZeroU32::new(d).ok_or(AppError::HoldDays))
            .transpose()?;

        let at = |text: Option<&str>| text.map(Timestamp::parse).transpose();
        let monitoring = Monitoring::new(at(monitor_from)?, at(monitor_until)?)?;

        Ok(self
            .ledger
            .add_source(&NewSource {
                person_id: PersonId::new(person),
                url,
                enabled: true,
                created_at: Timestamp::now(),
                priority,
                monitoring,
                hold_days,
            })
            .await?)
    }

    /// 人が URL を登録する。検知が取りこぼしたぶんを補う（#5）。
    ///
    /// # Errors
    ///
    /// URL が項目の形でない・種別を決められない・要るツールが無い・外へ出て失敗した・
    /// 台帳へ書けない、のどれか。
    pub async fn add_item(
        &self,
        source: i64,
        kind: Option<&str>,
        raw_url: &str,
    ) -> Result<ItemId, AppError> {
        let (url, hint) = url::normalize_item(raw_url)?;

        // 種別は正規化する前の入口から決める（#1）。入口が語らない形なら人に訊く。
        let content_type = match (hint, kind) {
            (_, Some(given)) => given.parse::<ContentType>()?,
            (TypeHint::Known(known), None) => known,
            (TypeHint::YoutubeUnknown, None) => return Err(AppError::UndecidableType),
        };

        // 中身を取るツールも #5 の対応表が決める。人が待っているので最優先で通す（#13）。
        let found = self
            .scheduler()?
            .describe(&url, content_type, Demand::interactive(Timestamp::now()))
            .await?;

        Ok(self
            .ledger
            .discover(
                &Discovered {
                    source_id: SourceId::new(source),
                    url: found.url,
                    published_at: found.published_at,
                    scheduled_start_at: found.scheduled_start_at,
                    content: found.content,
                    media: found.media,
                },
                Timestamp::now(),
            )
            .await?)
    }

    /// 項目を1つ、引き渡せる状態まで運ぶ。着いた状態を返す。
    ///
    /// # Errors
    ///
    /// 項目が無い・要るツールが無い・取得か文字起こしが失敗した、のどれか。
    pub async fn fetch(&self, id: i64) -> Result<State, AppError> {
        let id = ItemId::new(id);
        let item = self
            .ledger
            .item(id)
            .await?
            .ok_or(AppError::NoSuchItem(id))?
            .item;
        let scheduler = self.scheduler()?;
        let acquirer =
            scheduler.acquirer(item.content_type(), Demand::interactive(Timestamp::now()));

        // 預かる日数も期限も、取得が終わった瞬間に取得のスライスが決める（#1）。
        let state = Acquisition::new(&self.ledger, &self.paths.media_dir, acquirer.as_ref())
            .run(id)
            .await?;

        // スライスは互いを知らないので、**次に誰が拾うかは状態が決める**（#15）。
        // 巡回するエンジンは #33 なので、いまはここが状態を見て繋ぐ。
        let state = if state.name() == StateName::Transcribing {
            Transcription::new(&self.ledger, &self.paths.media_dir, scheduler.transcriber())
                .run(id)
                .await?
        } else {
            state
        };

        Ok(state)
    }

    /// 外部ツールと文字起こしモデルがどこで見つかるか。
    pub fn doctor(&self) -> Diagnosis {
        Diagnosis {
            tools: self.resolver.resolve_all(),
            transcribe_model_path: self.paths.transcribe_model.clone(),
            transcribe_model: Resolution::of_file(&self.paths.transcribe_model),
            directories: self.resolver.directories().to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 仕込んだ形（[`App::seeded`]）が、持ち主ごとに新しい順で返ること。
    ///
    /// 描いた結果は `escrow-gui` のテストが見る。ここは入口に依らない値のほう。
    #[tokio::test]
    async fn items_of_a_person_come_newest_first() {
        let media = tempfile::tempdir().unwrap();
        let app = App::seeded(media.path()).await;
        let persons = app.persons().await.unwrap();
        let owner = persons.iter().find(|p| p.name == "○○").unwrap();
        let nobody = persons.iter().find(|p| p.name == "□□").unwrap();

        let listed = app.items_of(owner.id).await.unwrap();
        let headlines: Vec<&str> = listed.iter().map(Listed::headline).collect();
        assert_eq!(headlines, ["○○の雑談配信", "明日の配信は21時から。"]);

        let states: Vec<&str> = listed.iter().map(Listed::state).collect();
        assert_eq!(states, ["holding", "kept"]);
        let kinds: Vec<&str> = listed.iter().map(Listed::content_type).collect();
        assert_eq!(kinds, ["youtube_live", "x_post"]);

        assert!(app.items_of(nobody.id).await.unwrap().is_empty());
    }
}

//! escrow の CLI。
//!
//! 外部向けは #4 の契約どおり `list` と `release` の2つ。それ以外は管理面で、
//! GUI（Phase 6）ができるまで手で回すための口。

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

use escrow_core::adapter::{
    Adapters, Resolver, Tool, gallerydl::GalleryDl, whisper::Whisper, ytdlp::YtDlp,
};
use escrow_core::config::{Config, Dirs, Paths};
use escrow_core::content::ContentType;
use escrow_core::handover;
use escrow_core::item::ItemId;
use escrow_core::pipeline::Pipeline;
use escrow_core::source::{PersonId, SourceId};
use escrow_core::state::{ReleaseReference, State, StateName};
use escrow_core::store::{NewItem, NewSource, Store};
use escrow_core::timestamp::Timestamp;
use escrow_core::url::{self, TypeHint};

/// 配信元から失われうるものを取り込み、手元に預かる。
#[derive(Debug, Parser)]
#[command(name = "escrow", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 引き渡せる項目を挙げる。
    List {
        /// 状態で絞る。
        #[arg(long)]
        state: Option<String>,
        /// 1件だけ見る。
        #[arg(long)]
        id: Option<i64>,
        /// JSON で出す。
        #[arg(long)]
        json: bool,
    },
    /// 外部が受け取り終えたことを伝える。手元の実体は消える。
    Release {
        id: i64,
        /// 移した先。escrow は解釈せずそのまま保管する。
        #[arg(long)]
        reference: Option<String>,
    },
    /// 配信元の持ち主。
    #[command(subcommand)]
    Person(PersonCommand),
    /// 監視対象。
    #[command(subcommand)]
    Source(SourceCommand),
    /// 項目。
    #[command(subcommand)]
    Item(ItemCommand),
    /// 項目を1つ、引き渡せる状態まで運ぶ。
    Fetch { id: i64 },
    /// 外部ツールがどこで見つかるかを出す。
    Doctor,
}

#[derive(Debug, Subcommand)]
enum PersonCommand {
    Add { name: String },
}

#[derive(Debug, Subcommand)]
enum SourceCommand {
    Add {
        #[arg(long)]
        person: i64,
        /// 不変 ID へ解決済みの URL。
        url: String,
        /// 新規投稿とライブ開始を確認する間隔。
        #[arg(long)]
        discover_interval_minutes: u32,
        /// 預かる日数。省くと捨てない。
        #[arg(long)]
        hold_days: Option<u32>,
    },
}

#[derive(Debug, Subcommand)]
enum ItemCommand {
    /// 人が URL を登録する。検知が取りこぼしたぶんを補う（#5）。
    Add {
        #[arg(long)]
        source: i64,
        /// URL から種別を決められないとき（`/watch?v=` など）に指定する。
        #[arg(long)]
        r#type: Option<String>,
        url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let app = App::open().await?;

    match cli.command {
        Command::List { state, id, json } => app.list(state, id, json).await,
        Command::Release { id, reference } => app.release(id, reference).await,
        Command::Person(PersonCommand::Add { name }) => app.add_person(&name).await,
        Command::Source(SourceCommand::Add {
            person,
            url,
            discover_interval_minutes,
            hold_days,
        }) => {
            app.add_source(person, &url, discover_interval_minutes, hold_days)
                .await
        }
        Command::Item(ItemCommand::Add {
            source,
            r#type,
            url,
        }) => app.add_item(source, r#type.as_deref(), &url).await,
        Command::Fetch { id } => app.fetch(id).await,
        Command::Doctor => app.doctor(),
    }
}

/// 設定と置き場所を解決した状態。
struct App {
    config: Config,
    paths: Paths,
    store: Store,
    resolver: Resolver,
}

impl App {
    async fn open() -> Result<Self> {
        let dirs = Dirs::discover().context("設定の置き場所を決められない")?;
        let config = Config::load(&dirs.config_file()).context("設定を読めない")?;
        let paths = Paths::resolve(&config, &dirs);
        let resolver = Resolver::from_env(&config.extra_paths(&dirs));

        let store = Store::open(&paths.db)
            .await
            .with_context(|| format!("DB を開けない: {}", paths.db.display()))?;

        Ok(Self {
            config,
            paths,
            store,
            resolver,
        })
    }

    /// 使えるツールを揃える。どれをいつ使うかは #5 の対応表（`Adapters`）が決める。
    fn adapters(&self) -> Result<Adapters> {
        let browser = self.config.auth.cookies_from;
        Ok(Adapters::new(
            YtDlp::new(self.tool(Tool::YtDlp)?, browser),
            GalleryDl::new(self.tool(Tool::GalleryDl)?, browser),
        ))
    }

    fn tool(&self, tool: Tool) -> Result<PathBuf> {
        self.resolver
            .resolve(tool)
            .path()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("{tool} が見つからない。`escrow doctor` で確かめる"))
    }

    async fn list(&self, state: Option<String>, id: Option<i64>, json: bool) -> Result<()> {
        let items = match (state, id) {
            (_, Some(id)) => self
                .store
                .item(ItemId::new(id))
                .await?
                .into_iter()
                .collect::<Vec<_>>(),
            (Some(name), None) => {
                let name: StateName = name.parse().context("知らない状態")?;
                self.store.items_in_state(name).await?
            }
            // #4 は状態を絞らない呼び方も許す。既定は引き渡し待ち。
            (None, None) => self.store.items_in_state(StateName::Kept).await?,
        };

        let handed: Vec<_> = items
            .iter()
            .map(|item| handover::handover(item, &self.paths.media_dir))
            .collect::<Result<_, _>>()?;

        if json {
            println!("{}", serde_json::to_string_pretty(&handed)?);
        } else {
            for entry in &handed {
                let headline = entry
                    .title
                    .as_deref()
                    .or(entry.body.as_deref())
                    .unwrap_or("");
                println!(
                    "{:>5}  {:<12} {:<15} {}",
                    entry.id, entry.state, entry.content_type, headline
                );
            }
        }
        Ok(())
    }

    async fn release(&self, id: i64, reference: Option<String>) -> Result<()> {
        let handed = handover::release(
            &self.store,
            &self.paths.media_dir,
            ItemId::new(id),
            reference.map(ReleaseReference::new),
        )
        .await?;

        println!("{}", serde_json::to_string_pretty(&handed)?);
        Ok(())
    }

    async fn add_person(&self, name: &str) -> Result<()> {
        println!("{}", self.store.add_person(name).await?);
        Ok(())
    }

    async fn add_source(
        &self,
        person: i64,
        raw_url: &str,
        interval: u32,
        hold_days: Option<u32>,
    ) -> Result<()> {
        let url = url::normalize_source(raw_url)?;
        let interval = NonZeroU32::new(interval).context("確認の間隔は1分以上")?;
        let hold_days = hold_days
            .map(|d| NonZeroU32::new(d).context("預かる日数は1日以上"))
            .transpose()?;

        let id = self
            .store
            .add_source(&NewSource {
                person_id: PersonId::new(person),
                url,
                enabled: true,
                created_at: Timestamp::now(),
                hold_days,
                discover_interval_minutes: interval,
            })
            .await?;

        println!("{id}");
        Ok(())
    }

    async fn add_item(&self, source: i64, kind: Option<&str>, raw_url: &str) -> Result<()> {
        let (url, hint) = url::normalize_item(raw_url)?;

        // 種別は正規化する前の入口から決める（#1）。入口が語らない形なら人に訊く。
        let content_type = match (hint, kind) {
            (_, Some(given)) => given.parse::<ContentType>()?,
            (TypeHint::Known(known), None) => known,
            (TypeHint::YoutubeUnknown, None) => bail!(
                "この URL からは種別を決められない。--type で指定する（youtube_video / \
                 youtube_live / youtube_shorts）"
            ),
        };

        // 中身を取るツールも #5 の対応表が決める。
        let adapters = self.adapters()?;
        let found = match content_type.media_type() {
            Some(media_type) => adapters.ytdlp.describe(&url, media_type).await?,
            // `Post` 側は `x_post` だけ。本文と繋がりは gallery-dl が返す。
            None => adapters.gallerydl.describe(&url).await?,
        };

        let id = self
            .store
            .add_item(&NewItem {
                source_id: SourceId::new(source),
                url: found.url,
                published_at: found.published_at,
                state: State::initial(found.media),
                state_since: Timestamp::now(),
                content: found.content,
            })
            .await?;

        println!("{id}");
        Ok(())
    }

    async fn fetch(&self, id: i64) -> Result<()> {
        let id = ItemId::new(id);
        let item = self
            .store
            .item(id)
            .await?
            .with_context(|| format!("項目 {id} が無い"))?;
        let source = self
            .store
            .source(item.source_id)
            .await?
            .context("配信元が無い")?;

        let adapters = self.adapters()?;
        // #5 の対応表が、この種別を取るのがどのツールかを決める。
        let acquirer = adapters.acquirer(item.content_type());
        let whisper = Whisper::new(
            self.tool(Tool::WhisperCli)?,
            self.tool(Tool::Ffmpeg)?,
            &self.paths.transcribe_model,
            self.config.transcribe.language.clone(),
        );

        let state = Pipeline::new(&self.store, &self.paths.media_dir, &acquirer, &whisper)
            .run(id, source.hold_policy())
            .await?;

        println!("{id} -> {state}", state = state.as_str());
        Ok(())
    }

    fn doctor(&self) -> Result<()> {
        for (tool, resolution) in self.resolver.resolve_all() {
            match resolution.path() {
                Some(path) => println!("  {tool:<12} {}  ✓", path.display()),
                None => println!("  {tool:<12} 見つかりません                 ✗"),
            }
        }

        let model = escrow_core::adapter::Resolution::of_file(&self.paths.transcribe_model);
        match model.path() {
            Some(path) => println!("\n  文字起こしモデル  {}  ✓", path.display()),
            None => println!(
                "\n  文字起こしモデル  {}  ✗",
                self.paths.transcribe_model.display()
            ),
        }

        println!("\n  探した場所");
        for dir in self.resolver.directories() {
            println!("    {}", dir.display());
        }
        Ok(())
    }
}

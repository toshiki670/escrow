//! escrow の CLI。
//!
//! 外部向けは #4 の契約どおり `list` と `release` の2つ。それ以外は管理のコマンドで、
//! GUI（Phase 8）ができるまで手で回すための入口。
//!
//! イベントストアを開く手順と読む・書く関数は `escrow-app` が持つ（#82）。ここに在るのは
//! 引数の受け取りと、出力の形だけ。

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};

use escrow_app::{App, AppError};

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
        /// 検知の重み。大きいほど予算の分け前が増える（#13）。
        #[arg(long, default_value = "1")]
        priority: u32,
        /// 監視を始める日時。--monitor-until と対で指定する。
        #[arg(long)]
        monitor_from: Option<String>,
        /// 監視を終える日時。両方省くと区切らず監視する。
        #[arg(long)]
        monitor_until: Option<String>,
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
        Command::List { state, id, json } => list(&app, state.as_deref(), id, json).await,
        Command::Release { id, reference } => {
            let handed = app.release(id, reference.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&handed)?);
            Ok(())
        }
        Command::Person(PersonCommand::Add { name }) => {
            println!("{}", app.add_person(&name).await?);
            Ok(())
        }
        Command::Source(SourceCommand::Add {
            person,
            url,
            priority,
            monitor_from,
            monitor_until,
            hold_days,
        }) => {
            let id = app
                .add_source(
                    person,
                    &url,
                    priority,
                    monitor_from.as_deref(),
                    monitor_until.as_deref(),
                    hold_days,
                )
                .await?;
            println!("{id}");
            Ok(())
        }
        Command::Item(ItemCommand::Add {
            source,
            r#type,
            url,
        }) => {
            let id = app
                .add_item(source, r#type.as_deref(), &url)
                .await
                .map_err(hinted)?;
            println!("{id}");
            Ok(())
        }
        Command::Fetch { id } => {
            let state = app.fetch(id).await.map_err(hinted)?;
            println!("{id} -> {state}", state = state.as_str());
            Ok(())
        }
        Command::Doctor => {
            doctor(&app);
            Ok(())
        }
    }
}

/// 直し方が CLI の語になる失敗に、その語を足す。
///
/// ツールが無いなら `escrow doctor`、種別を決められないなら `--type`。どちらも
/// この入口の名前なので、足すのはここ。
fn hinted(error: AppError) -> anyhow::Error {
    match error {
        AppError::MissingTool(_) => anyhow!("{error}。`escrow doctor` で確かめる"),
        AppError::UndecidableType => {
            anyhow!("{error}。--type で指定する（youtube_video / youtube_live / youtube_shorts）")
        }
        _ => error.into(),
    }
}

/// #4 の `list`。文の表では、見出しに `title` か `body` の全文をそのまま出す。
///
/// `escrow-app` の `items_of` は `body` の1行目を見出しにするが、#4 の出力の形は
/// そのまま（#82）。
async fn list(app: &App, state: Option<&str>, id: Option<i64>, json: bool) -> Result<()> {
    let handed = app.list(state, id).await?;

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

fn doctor(app: &App) {
    let diagnosis = app.doctor();

    for (tool, resolution) in &diagnosis.tools {
        match resolution.path() {
            Some(path) => println!("  {tool:<12} {}  ✓", path.display()),
            None => println!("  {tool:<12} 見つかりません                 ✗"),
        }
    }

    match diagnosis.transcribe_model.path() {
        Some(path) => println!("\n  文字起こしモデル  {}  ✓", path.display()),
        None => println!(
            "\n  文字起こしモデル  {}  ✗",
            diagnosis.transcribe_model_path.display()
        ),
    }

    println!("\n  探した場所");
    for dir in &diagnosis.directories {
        println!("    {}", dir.display());
    }
}

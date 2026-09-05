//! 事象の追記と投影（#15）。
//!
//! 唯一の真実は `item_event` で、追記しかしない。`item` はそこから作られる投影で、
//! **いつでも捨てて作り直せる**。読むのは投影、書くのは事象、という分け方（CQRS）。
//!
//! **公開するのは [`Ledger::discover`] と [`Ledger::append`] の2つだけ。** 投影は
//! その2つを通ってしか動かないので、ログと投影がずれる書き方がそもそも書けない。
//! 何を公開してよいかは `tests/public_api.rs` の表が決める。
//!
//! 集約でディレクトリを切っていて、いまは `item` だけ。このファイルには集約に
//! 依存しない仕組み — 接続・番号・行を読むときの失敗 — を置く。

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use escrow_domain::content::ContentType;
use escrow_domain::item::ItemId;
use escrow_domain::source::MonitoringError;
use escrow_domain::state::{EventKind, IllegalTransition, StateName};
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::{self, NormalizedUrl};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Executor, SqlitePool};
use thiserror::Error;

mod catalog;
mod item;
mod rebuild;
#[cfg(test)]
mod testing;

pub use catalog::NewSource;
pub use item::{Log, Projected, Recorded};

/// `migrations/` をバイナリへ埋め込む。前進のみで、down migration は持たない（#7）。
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// 投影の DDL。理由は `projections/item.sql` に在る。
const PROJECTION: &str = include_str!("../projections/item.sql");

/// 項目の中での事象の通し番号。1 から始まり、1 は必ず `discovered`（#1）。
///
/// 追記のときに次の番号を指定するので、読んでから書くまでに誰かが動かしていれば
/// `(item_id, seq)` の UNIQUE が弾く。#7 が「毎回 `WHERE` 句を書く」という規律で
/// 守っていたものが、**忘れられない制約**になる（#15）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(NonZeroU32);

impl Seq {
    /// 誕生の番号。ログの先頭は必ずこれで、`discovered` が入る。
    pub const FIRST: Self = Self(NonZeroU32::MIN);

    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// 次に書く番号。
    ///
    /// 上限で頭打ちになるが、そこで止まっても**同じ番号を二度書くことになるので
    /// UNIQUE が弾く**。43 億件目に届かない上限を `Option` で持ち回るより、
    /// 既にある制約に落ちてもらうほうが呼ぶ側が素直になる。
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// DB から読み戻す。範囲の検査は呼ぶ側（列は `INTEGER`）。
    pub(crate) const fn from_stored(seq: NonZeroU32) -> Self {
        Self(seq)
    }
}

/// 台帳を触れなかったとき。
#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("DB へアクセスできない")]
    Database(#[from] sqlx::Error),
    #[error("DB の置き場所を作れない: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("DB の移行に失敗した")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("DB の行を読めない")]
    Row(#[from] RowError),
    #[error(transparent)]
    IllegalTransition(#[from] IllegalTransition),
    #[error("項目 {0} が無い")]
    NoSuchItem(ItemId),
    /// 読んでから書くまでに、誰かが事象を足していた。読み直して決め直す（#7）。
    #[error("読んだときから事象が増えている。読み直して決め直す")]
    Superseded,
}

impl LedgerError {
    /// `(item_id, seq)` の UNIQUE に当たったなら、競合として読み替える。
    fn from_append(error: sqlx::Error) -> Self {
        match &error {
            sqlx::Error::Database(db) if db.is_unique_violation() => Self::Superseded,
            _ => Self::Database(error),
        }
    }
}

/// 行をドメイン型へ写せなかったとき。
///
/// #1 が「`NULL` を許すのはこの列だけ」と決めたぶんが、ここで実際に効く。
/// 握りつぶすと台帳が静かに壊れるので、名前を付けて外へ出す。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RowError {
    #[error("item {id}: 知らない種別 `{value}`")]
    UnknownContentType { id: i64, value: String },
    #[error("item {id}: 知らない状態 `{value}`")]
    UnknownState { id: i64, value: String },
    #[error("item {id}: 種別 {content_type} は {column} を要るが NULL")]
    MissingColumn {
        id: i64,
        content_type: ContentType,
        column: &'static str,
    },
    #[error("item {id}: 種別 {content_type} に {column} は無いはずだが値が入っている")]
    UnexpectedColumn {
        id: i64,
        content_type: ContentType,
        column: &'static str,
    },
    #[error("item {id}: 状態 {state} に {column} は無いはずだが値が入っている")]
    ColumnWithoutState {
        id: i64,
        state: StateName,
        column: &'static str,
    },
    #[error("item {id}: 状態 {state} は {column} を要るが NULL")]
    StateMissingColumn {
        id: i64,
        state: StateName,
        column: &'static str,
    },
    #[error("item {id}: {column} が正規形でない `{value}`")]
    UnnormalizedUrl {
        id: i64,
        column: &'static str,
        value: String,
    },
    #[error("item {id}: {column} を日時として読めない `{value}`")]
    BadTimestamp {
        id: i64,
        column: &'static str,
        value: String,
    },
    #[error("source {id}: url が正規形でない `{value}`")]
    UnnormalizedSourceUrl { id: i64, value: String },
    #[error("source {id}: {column} を日時として読めない `{value}`")]
    BadSourceTimestamp {
        id: i64,
        column: &'static str,
        value: String,
    },
    #[error("source {id}: {source}")]
    BadMonitoring {
        id: i64,
        #[source]
        source: MonitoringError,
    },
    #[error("{table} {id}: {column} が範囲外 `{value}`")]
    OutOfRange {
        table: &'static str,
        id: i64,
        column: &'static str,
        value: i64,
    },
    #[error("exclude {id}: 知らない種別 `{value}`")]
    UnknownExcludeType { id: i64, value: String },
    #[error("item {id}: 知らない事象 `{value}`")]
    UnknownEventKind { id: i64, value: String },
    #[error("item {id}: 事象 {kind} は {column} を要るが NULL")]
    EventMissingColumn {
        id: i64,
        kind: EventKind,
        column: &'static str,
    },
    #[error("item {id}: ログの先頭が discovered でない")]
    LogDoesNotBegin { id: i64 },
    #[error("item {id}: seq が {expected} のはずが {actual}")]
    LogOutOfOrder { id: i64, expected: u32, actual: i64 },
}

/// SQLite への接続。事象を追記し、投影を保つ。
#[derive(Debug)]
pub struct Ledger {
    pool: SqlitePool,
}

impl Ledger {
    /// 既存の DB を開くか、無ければ作って移行を当てる。
    pub async fn open(path: &Path) -> Result<Self, LedgerError> {
        // `create_if_missing` はファイルを作るが、ディレクトリは作らない。
        // 初回起動では置き場所ごと無いので、ここで用意する。
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| LedgerError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            // 別プロセスの書き込みとぶつかったら、失敗させず待つ。
            .busy_timeout(std::time::Duration::from_secs(10));

        Self::connect(options).await
    }

    /// テスト用。プロセスが終われば消える。
    pub async fn open_in_memory() -> Result<Self, LedgerError> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);

        Self::connect(options).await
    }

    async fn connect(options: SqliteConnectOptions) -> Result<Self, LedgerError> {
        // 書き込みは1本に絞る。SQLite は同時に1つの writer しか許さないので、
        // プールを広げても待ちが増えるだけ。
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;

        // 適用済みでバイナリ側に無い移行があれば VersionMissing で失敗する。
        // 古いバイナリが新しい DB を開く事故（brew の降格）はここで止まる。
        MIGRATOR.run(&pool).await?;

        // 投影は移行に載らないので、無ければここで作る。DDL の写しは
        // projections/item.sql の1つきりで、rebuild も同じものを流す。
        pool.execute(PROJECTION).await?;

        Ok(Self { pool })
    }
}

// ---------------------------------------------------------------- 行を読む

/// 保存されている URL が正規形のままか確かめる。
///
/// 正規化を通した値しか入れないので、ずれていたら手で書き換えられたか、
/// 正規化の規則が変わったかのどちらか。黙って新しい形へ読み替えると `UNIQUE` と
/// 食い違うので、はっきり落とす。
fn normalized(id: i64, column: &'static str, value: &str) -> Result<NormalizedUrl, RowError> {
    let bad = || RowError::UnnormalizedUrl {
        id,
        column,
        value: value.to_owned(),
    };

    let (canonical, _) = url::normalize_item(value).map_err(|_| bad())?;
    if canonical.as_str() == value {
        Ok(canonical)
    } else {
        Err(bad())
    }
}

fn timestamp(id: i64, column: &'static str, value: &str) -> Result<Timestamp, RowError> {
    Timestamp::parse(value).map_err(|_| RowError::BadTimestamp {
        id,
        column,
        value: value.to_owned(),
    })
}

/// `source` の日時の列。`NULL` はそのまま空として返す。
fn source_timestamp(
    id: i64,
    column: &'static str,
    text: Option<&str>,
) -> Result<Option<Timestamp>, RowError> {
    text.map(|text| {
        Timestamp::parse(text).map_err(|_| RowError::BadSourceTimestamp {
            id,
            column,
            value: text.to_owned(),
        })
    })
    .transpose()
}

fn positive(
    value: Option<i64>,
    table: &'static str,
    id: i64,
    column: &'static str,
) -> Result<Option<NonZeroU32>, RowError> {
    value
        .map(|v| {
            u32::try_from(v)
                .ok()
                .and_then(NonZeroU32::new)
                .ok_or(RowError::OutOfRange {
                    table,
                    id,
                    column,
                    value: v,
                })
        })
        .transpose()
}

fn content_type_of(id: i64, value: &str) -> Result<ContentType, RowError> {
    ContentType::from_str(value).map_err(|_| RowError::UnknownContentType {
        id,
        value: value.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use escrow_domain::state::StateName;

    use crate::testing::seeded;
    use crate::{Ledger, LedgerError, Seq};

    #[tokio::test]
    async fn migrations_apply_to_an_empty_database() {
        // open_in_memory が移行と投影の作成まで通す。通らなければここで落ちる。
        let ledger = Ledger::open_in_memory().await.unwrap();
        assert!(
            ledger
                .items_in_state(StateName::Kept)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 開き直しても投影が二重に作られないこと。
    ///
    /// 投影は移行に載らないので、`_sqlx_migrations` は「もう作った」を覚えていない。
    /// 覚えていない側で `IF NOT EXISTS` が効いていることを確かめる。
    #[tokio::test]
    async fn opening_an_existing_database_keeps_its_projection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("escrow.db");

        let ledger = Ledger::open(&path).await.unwrap();
        let source = crate::testing::seed_into(&ledger).await;
        let id = crate::testing::a_holding_item(&ledger, source).await;
        drop(ledger);

        let ledger = Ledger::open(&path).await.unwrap();
        assert!(ledger.item(id).await.unwrap().is_some(), "投影が残っている");
    }

    /// 古いバイナリが新しい DB を開いたら止まること。
    ///
    /// 配布が `brew upgrade` なので降格でこれが起きる。sqlx が既定で
    /// `VersionMissing` にするので、こちらで手当てを書かなくてよい（#7）。
    #[tokio::test]
    async fn refuses_a_database_from_a_newer_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("escrow.db");

        let ledger = Ledger::open(&path).await.unwrap();
        // このバイナリが知らない移行が当たっている、という体。
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, installed_on, success, checksum, execution_time) \
             VALUES (999, 'from the future', datetime('now'), 1, X'00', 0)",
        )
        .execute(&ledger.pool)
        .await
        .unwrap();
        drop(ledger);

        match Ledger::open(&path).await {
            Err(LedgerError::Migrate(sqlx::migrate::MigrateError::VersionMissing(999))) => {}
            other => panic!("降格を止めなかった: {other:?}"),
        }
    }

    /// 初回起動では置き場所ごと無い。ディレクトリから作ること。
    #[tokio::test]
    async fn opening_creates_the_directory_it_needs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Application Support/escrow/escrow.db");
        assert!(!path.parent().unwrap().exists());

        let ledger = Ledger::open(&path).await.unwrap();
        assert!(path.is_file());
        assert!(
            ledger
                .items_in_state(StateName::Kept)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 番号は 1 から始まり、飛ばない。
    #[tokio::test]
    async fn the_log_begins_at_one() {
        let (ledger, source) = seeded().await;
        let id = crate::testing::a_holding_item(&ledger, source).await;

        let log = ledger.log(id).await.unwrap().unwrap();
        assert_eq!(log.rest.len(), 2);
        assert_eq!(log.rest[0].seq, Seq::FIRST.next());
        assert_eq!(log.rest[1].seq.get(), 3);
    }
}

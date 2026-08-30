//! SQLite への読み書き。
//!
//! DB は #1 の erDiagram のとおり平らなカラムを持つだけで、`CHECK` 制約も置かない。
//! 「`Media` に `body` は無い」「`release_reference` は `released` だけが持つ」は、
//! **行をドメイン型へ写すときに parse で効かせる**。境界はここ1か所。
//!
//! 書き込みの規律（#7）:
//! - WAL。`escrow release`（CLI）とエンジンは別プロセスで、両方が書く
//! - 状態遷移は compare-and-swap。期待した状態のときだけ書き、影響行数 0 なら
//!   誰かが先に動かしているので読み直す
//! - `release` と `discarded` は DB を先に更新し、ファイルは後で消す

use std::num::NonZeroU32;
use std::path::Path;
use std::str::FromStr;

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use thiserror::Error;

use crate::content::{Content, ContentType};
use crate::item::{Item, ItemId};
use crate::source::{Exclude, ExcludeId, Person, PersonId, Source, SourceId};
use crate::state::{self, Event, IllegalTransition, ReleaseReference, State, StateName};
use crate::timestamp::Timestamp;
use crate::url::{self, NormalizedUrl};

/// `migrations/` をバイナリへ埋め込む。前進のみで、down migration は持たない。
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("DB へアクセスできない")]
    Database(#[from] sqlx::Error),
    #[error("DB の移行に失敗した")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("DB の行を読めない")]
    Row(#[from] RowError),
    #[error(transparent)]
    IllegalTransition(#[from] IllegalTransition),
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
    #[error("item {id}: 状態 {state} に release_reference は無いはずだが値が入っている")]
    ReferenceWithoutRelease { id: i64, state: StateName },
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
    #[error("source {id}: created_at を日時として読めない `{value}`")]
    BadSourceTimestamp { id: i64, value: String },
    #[error("{table} {id}: {column} が範囲外 `{value}`")]
    OutOfRange {
        table: &'static str,
        id: i64,
        column: &'static str,
        value: i64,
    },
    #[error("exclude {id}: 知らない種別 `{value}`")]
    UnknownExcludeType { id: i64, value: String },
}

#[derive(Debug)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// 既存の DB を開くか、無ければ作って移行を当てる。
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
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
    pub async fn open_in_memory() -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);

        Self::connect(options).await
    }

    async fn connect(options: SqliteConnectOptions) -> Result<Self, StoreError> {
        // 書き込みは1本に絞る。SQLite は同時に1つの writer しか許さないので、
        // プールを広げても待ちが増えるだけ。読み出し用を分けるのは Phase 4（#7）。
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;

        // 適用済みでバイナリ側に無い移行があれば VersionMissing で失敗する。
        // 古いバイナリが新しい DB を開く事故（brew の降格）はここで止まる。
        MIGRATOR.run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

// ---------------------------------------------------------------- person

impl Store {
    pub async fn add_person(&self, name: &str) -> Result<PersonId, StoreError> {
        let id = sqlx::query!(
            "INSERT INTO person (name) VALUES (?) RETURNING id AS \"id!\"",
            name
        )
        .fetch_one(&self.pool)
        .await?
        .id;

        Ok(PersonId::new(id))
    }

    pub async fn person(&self, id: PersonId) -> Result<Option<Person>, StoreError> {
        let key = id.get();
        let row = sqlx::query!(r#"SELECT id AS "id!", name FROM person WHERE id = ?"#, key)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|row| Person {
            id: PersonId::new(row.id),
            name: row.name,
        }))
    }
}

// ---------------------------------------------------------------- source

/// 配信元を登録するときに渡すもの。`id` はまだ無い。
#[derive(Debug, Clone)]
pub struct NewSource {
    pub person_id: PersonId,
    pub url: NormalizedUrl,
    pub enabled: bool,
    pub created_at: Timestamp,
    pub hold_days: Option<NonZeroU32>,
    pub discover_interval_minutes: NonZeroU32,
}

impl Store {
    pub async fn add_source(&self, source: &NewSource) -> Result<SourceId, StoreError> {
        let person_id = source.person_id.get();
        let url = source.url.as_str();
        let enabled = i64::from(source.enabled);
        let created_at = source.created_at.to_text();
        let hold_days = source.hold_days.map(|d| i64::from(d.get()));
        let interval = i64::from(source.discover_interval_minutes.get());

        let id = sqlx::query!(
            "INSERT INTO source (person_id, url, enabled, created_at, hold_days, \
             discover_interval_minutes) VALUES (?, ?, ?, ?, ?, ?) RETURNING id AS \"id!\"",
            person_id,
            url,
            enabled,
            created_at,
            hold_days,
            interval,
        )
        .fetch_one(&self.pool)
        .await?
        .id;

        Ok(SourceId::new(id))
    }

    pub async fn source(&self, id: SourceId) -> Result<Option<Source>, StoreError> {
        let key = id.get();
        let row = sqlx::query!(
            r#"SELECT id AS "id!", person_id, url, enabled, created_at, hold_days,
                      discover_interval_minutes
               FROM source WHERE id = ?"#,
            key
        )
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else { return Ok(None) };

        let url = url::normalize_source(&row.url)
            .ok()
            .filter(|u| u.as_str() == row.url);
        let url = url.ok_or(RowError::UnnormalizedSourceUrl {
            id: row.id,
            value: row.url.clone(),
        })?;

        Ok(Some(Source {
            id: SourceId::new(row.id),
            person_id: PersonId::new(row.person_id),
            url,
            enabled: row.enabled != 0,
            created_at: Timestamp::parse(&row.created_at).map_err(|_| {
                RowError::BadSourceTimestamp {
                    id: row.id,
                    value: row.created_at.clone(),
                }
            })?,
            hold_days: positive(row.hold_days, "source", row.id, "hold_days")?,
            discover_interval_minutes: positive(
                Some(row.discover_interval_minutes),
                "source",
                row.id,
                "discover_interval_minutes",
            )?
            .ok_or(RowError::OutOfRange {
                table: "source",
                id: row.id,
                column: "discover_interval_minutes",
                value: row.discover_interval_minutes,
            })?,
        }))
    }

    pub async fn add_exclude(
        &self,
        source_id: Option<SourceId>,
        content_type: ContentType,
        enabled: bool,
    ) -> Result<ExcludeId, StoreError> {
        let scoped = source_id.map(SourceId::get);
        let value = content_type.as_str();
        let enabled = i64::from(enabled);

        let id = sqlx::query!(
            "INSERT INTO exclude (source_id, content_type, enabled) VALUES (?, ?, ?) RETURNING id AS \"id!\"",
            scoped,
            value,
            enabled,
        )
        .fetch_one(&self.pool)
        .await?
        .id;

        Ok(ExcludeId::new(id))
    }

    /// 全除外条件。当たり判定は [`Exclude::covers`] が持つ。
    pub async fn excludes(&self) -> Result<Vec<Exclude>, StoreError> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", source_id, content_type, enabled FROM exclude ORDER BY id"#
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Exclude {
                    id: ExcludeId::new(row.id),
                    source_id: row.source_id.map(SourceId::new),
                    content_type: ContentType::from_str(&row.content_type).map_err(|_| {
                        RowError::UnknownExcludeType {
                            id: row.id,
                            value: row.content_type.clone(),
                        }
                    })?,
                    enabled: row.enabled != 0,
                })
            })
            .collect()
    }
}

// ---------------------------------------------------------------- item

/// 見つけた項目。`id` はまだ無い。
#[derive(Debug, Clone)]
pub struct NewItem {
    pub source_id: SourceId,
    pub url: NormalizedUrl,
    pub published_at: Timestamp,
    pub state: State,
    pub state_since: Timestamp,
    pub content: Content,
}

impl Store {
    pub async fn add_item(&self, item: &NewItem) -> Result<ItemId, StoreError> {
        let columns = ItemColumns::from(&item.content, &item.state);

        let source_id = item.source_id.get();
        let url = item.url.as_str();
        let content_type = item.content.content_type().as_str();
        let published_at = item.published_at.to_text();
        let state = item.state.as_str();
        let state_since = item.state_since.to_text();

        let id = sqlx::query!(
            "INSERT INTO item (source_id, url, content_type, published_at, state, state_since, \
             title, body, in_reply_to_url, quoted_url, release_reference) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id AS \"id!\"",
            source_id,
            url,
            content_type,
            published_at,
            state,
            state_since,
            columns.title,
            columns.body,
            columns.in_reply_to_url,
            columns.quoted_url,
            columns.release_reference,
        )
        .fetch_one(&self.pool)
        .await?
        .id;

        Ok(ItemId::new(id))
    }

    pub async fn item(&self, id: ItemId) -> Result<Option<Item>, StoreError> {
        let key = id.get();
        let row = sqlx::query_as!(
            ItemRow,
            r#"SELECT id AS "id!", source_id, url, content_type, published_at, state,
                      state_since, title, body, in_reply_to_url, quoted_url, release_reference
               FROM item WHERE id = ?"#,
            key
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(Item::try_from).transpose().map_err(StoreError::Row)
    }

    /// 自然キーで引く。同じ項目を二重に登録しないための入口。
    pub async fn item_by_url(&self, url: &NormalizedUrl) -> Result<Option<Item>, StoreError> {
        let key = url.as_str();
        let row = sqlx::query_as!(
            ItemRow,
            r#"SELECT id AS "id!", source_id, url, content_type, published_at, state,
                      state_since, title, body, in_reply_to_url, quoted_url, release_reference
               FROM item WHERE url = ?"#,
            key
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(Item::try_from).transpose().map_err(StoreError::Row)
    }

    /// ある状態の項目を集める。`holding` をまとめて回す、`list --state kept`。
    pub async fn items_in_state(&self, state: StateName) -> Result<Vec<Item>, StoreError> {
        let key = state.as_str();
        let rows = sqlx::query_as!(
            ItemRow,
            r#"SELECT id AS "id!", source_id, url, content_type, published_at, state,
                      state_since, title, body, in_reply_to_url, quoted_url, release_reference
               FROM item WHERE state = ? ORDER BY id"#,
            key
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(Item::try_from)
            .collect::<Result<_, _>>()
            .map_err(StoreError::Row)
    }

    /// 事象を1つ適用する。
    ///
    /// 次の状態は [`crate::state::next`] が決める。**次の状態を直接渡す口は無い**ので、
    /// 図に無い遷移を DB へ書くことができない。
    ///
    /// 書き込みは compare-and-swap で、`from` が今も DB の状態と一致するときだけ通る。
    /// 一致しなければ [`Applied::Superseded`] を返す — `escrow release` とエンジンは
    /// 別プロセスなので、誰かが先に動かしていることがある。読み直して決め直す。
    pub async fn apply(
        &self,
        id: ItemId,
        from: &State,
        event: &Event,
        now: Timestamp,
    ) -> Result<Applied, StoreError> {
        let next = state::next(from, event)?;

        let key = id.get();
        let expected = from.as_str();
        let state = next.as_str();
        let state_since = now.to_text();
        let reference = match &next {
            State::Released { reference } => reference.as_ref().map(|r| r.as_str().to_owned()),
            _ => None,
        };

        let affected = sqlx::query!(
            "UPDATE item SET state = ?, state_since = ?, release_reference = ? \
             WHERE id = ? AND state = ?",
            state,
            state_since,
            reference,
            key,
            expected,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();

        Ok(if affected == 1 {
            Applied::Written(next)
        } else {
            Applied::Superseded
        })
    }
}

/// [`Store::apply`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// 書けた。
    Written(State),
    /// 読んだときから状態が動いていたので書かなかった。読み直して決め直す。
    Superseded,
}

// ---------------------------------------------------------------- 行との往復

/// `item` の1行そのまま。ここから先はドメイン型なので、この形は外へ出さない。
struct ItemRow {
    id: i64,
    source_id: i64,
    url: String,
    content_type: String,
    published_at: String,
    state: String,
    state_since: String,
    title: Option<String>,
    body: Option<String>,
    in_reply_to_url: Option<String>,
    quoted_url: Option<String>,
    release_reference: Option<String>,
}

/// ドメイン型から、`NULL` を許す列だけを取り出したもの。
struct ItemColumns {
    title: Option<String>,
    body: Option<String>,
    in_reply_to_url: Option<String>,
    quoted_url: Option<String>,
    release_reference: Option<String>,
}

impl ItemColumns {
    fn from(content: &Content, state: &State) -> Self {
        let (title, body, in_reply_to_url, quoted_url) = match content {
            Content::Media { title, .. } => (Some(title.clone()), None, None, None),
            Content::Post {
                body,
                in_reply_to,
                quoted,
            } => (
                None,
                Some(body.clone()),
                in_reply_to.as_ref().map(|u| u.as_str().to_owned()),
                quoted.as_ref().map(|u| u.as_str().to_owned()),
            ),
        };

        Self {
            title,
            body,
            in_reply_to_url,
            quoted_url,
            release_reference: match state {
                State::Released { reference } => reference.as_ref().map(|r| r.as_str().to_owned()),
                _ => None,
            },
        }
    }
}

impl TryFrom<ItemRow> for Item {
    type Error = RowError;

    fn try_from(row: ItemRow) -> Result<Self, Self::Error> {
        let id = row.id;
        let content_type =
            ContentType::from_str(&row.content_type).map_err(|_| RowError::UnknownContentType {
                id,
                value: row.content_type.clone(),
            })?;

        let content = content_from_row(&row, content_type)?;
        let state = state_from_row(&row)?;

        Ok(Self {
            id: ItemId::new(id),
            source_id: SourceId::new(row.source_id),
            url: normalized(id, "url", &row.url)?,
            published_at: timestamp(id, "published_at", &row.published_at)?,
            state,
            state_since: timestamp(id, "state_since", &row.state_since)?,
            content,
        })
    }
}

/// 種別が `Media` 側か `Post` 側かで、要る列と、あってはいけない列が決まる。
fn content_from_row(row: &ItemRow, content_type: ContentType) -> Result<Content, RowError> {
    let id = row.id;
    let missing = |column| RowError::MissingColumn {
        id,
        content_type,
        column,
    };
    let unexpected = |column| RowError::UnexpectedColumn {
        id,
        content_type,
        column,
    };

    match content_type.media_type() {
        Some(media_type) => {
            let title = row.title.clone().ok_or_else(|| missing("title"))?;
            // Media は body も繋がりの URL も持たない（#1）。
            if row.body.is_some() {
                return Err(unexpected("body"));
            }
            if row.in_reply_to_url.is_some() {
                return Err(unexpected("in_reply_to_url"));
            }
            if row.quoted_url.is_some() {
                return Err(unexpected("quoted_url"));
            }
            Ok(Content::Media { media_type, title })
        }
        None => {
            let body = row.body.clone().ok_or_else(|| missing("body"))?;
            if row.title.is_some() {
                return Err(unexpected("title"));
            }
            Ok(Content::Post {
                body,
                in_reply_to: row
                    .in_reply_to_url
                    .as_deref()
                    .map(|u| normalized(id, "in_reply_to_url", u))
                    .transpose()?,
                quoted: row
                    .quoted_url
                    .as_deref()
                    .map(|u| normalized(id, "quoted_url", u))
                    .transpose()?,
            })
        }
    }
}

/// 状態は `state` と `release_reference` の2列が揃って初めて決まる。
fn state_from_row(row: &ItemRow) -> Result<State, RowError> {
    let id = row.id;
    let name = StateName::from_str(&row.state).map_err(|_| RowError::UnknownState {
        id,
        value: row.state.clone(),
    })?;

    if name != StateName::Released && row.release_reference.is_some() {
        return Err(RowError::ReferenceWithoutRelease { id, state: name });
    }

    Ok(match name {
        StateName::Waiting => State::Waiting,
        StateName::Acquiring => State::Acquiring,
        StateName::Transcribing => State::Transcribing,
        StateName::Holding => State::Holding,
        StateName::Kept => State::Kept,
        StateName::Discarded => State::Discarded,
        StateName::Released => State::Released {
            reference: row.release_reference.clone().map(ReleaseReference::new),
        },
        StateName::Deleted => State::Deleted,
        StateName::Error => State::Error,
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::MediaType;
    use crate::url::normalize_item;

    fn url(raw: &str) -> NormalizedUrl {
        normalize_item(raw).expect(raw).0
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    /// 人と配信元を1つずつ持つ DB。項目は FK を要るので、これが最小の土台。
    async fn seeded() -> (Store, SourceId) {
        let store = Store::open_in_memory().await.unwrap();
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
                hold_days: NonZeroU32::new(7),
                discover_interval_minutes: NonZeroU32::new(15).unwrap(),
            })
            .await
            .unwrap();
        (store, source)
    }

    fn media_item(source_id: SourceId, state: State) -> NewItem {
        NewItem {
            source_id,
            url: url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            published_at: at("2026-03-01T20:00:00+09:00"),
            state,
            state_since: at("2026-03-01T22:30:00+09:00"),
            content: Content::Media {
                media_type: MediaType::YoutubeLive,
                title: "○○の雑談配信".to_owned(),
            },
        }
    }

    fn post_item(source_id: SourceId) -> NewItem {
        NewItem {
            source_id,
            url: url("https://x.com/jack/status/20"),
            published_at: at("2026-03-01T12:00:00+09:00"),
            state: State::Kept,
            state_since: at("2026-03-01T12:01:00+09:00"),
            content: Content::Post {
                body: "明日の配信は21時から。".to_owned(),
                in_reply_to: Some(url("https://x.com/someone/status/19")),
                quoted: Some(url("https://youtu.be/dQw4w9WgXcQ")),
            },
        }
    }

    #[tokio::test]
    async fn migrations_apply_to_an_empty_database() {
        // open_in_memory が移行まで通す。通らなければここで落ちる。
        let store = Store::open_in_memory().await.unwrap();
        assert!(
            store
                .items_in_state(StateName::Kept)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn round_trips_a_media_item() {
        let (store, source) = seeded().await;
        let new = media_item(source, State::Holding);
        let id = store.add_item(&new).await.unwrap();

        let read = store.item(id).await.unwrap().expect("入れたものが読める");
        assert_eq!(read.id, id);
        assert_eq!(read.source_id, source);
        assert_eq!(read.url, new.url);
        assert_eq!(read.published_at, new.published_at);
        assert_eq!(read.state, new.state);
        assert_eq!(read.state_since, new.state_since);
        assert_eq!(read.content, new.content);
        assert_eq!(read.content_type(), ContentType::YoutubeLive);
    }

    #[tokio::test]
    async fn round_trips_a_post_with_its_links() {
        let (store, source) = seeded().await;
        let new = post_item(source);
        let id = store.add_item(&new).await.unwrap();

        let read = store.item(id).await.unwrap().unwrap();
        assert_eq!(read.content, new.content);
        assert_eq!(read.content_type(), ContentType::XPost);
    }

    /// `release_reference` は `released` と対でしか意味を持たない（#1）。
    #[tokio::test]
    async fn round_trips_a_release_reference() {
        let (store, source) = seeded().await;
        let reference = ReleaseReference::new("Attachments/2026-03-01 ○○.mp4");
        let new = media_item(
            source,
            State::Released {
                reference: Some(reference.clone()),
            },
        );
        let id = store.add_item(&new).await.unwrap();

        let read = store.item(id).await.unwrap().unwrap();
        assert_eq!(
            read.state,
            State::Released {
                reference: Some(reference)
            }
        );
    }

    #[tokio::test]
    async fn finds_an_item_by_its_natural_key() {
        let (store, source) = seeded().await;
        let new = media_item(source, State::Kept);
        let id = store.add_item(&new).await.unwrap();

        // 入口が違っても同じ正規形になるので、同じ行に着く。
        let found = store
            .item_by_url(&url("https://youtu.be/dQw4w9WgXcQ?si=xyz"))
            .await
            .unwrap()
            .expect("正規化して引ける");
        assert_eq!(found.id, id);
    }

    #[tokio::test]
    async fn the_same_url_cannot_be_stored_twice() {
        let (store, source) = seeded().await;
        let new = media_item(source, State::Kept);
        store.add_item(&new).await.unwrap();

        assert!(store.add_item(&new).await.is_err(), "UNIQUE が効くこと");
    }

    #[tokio::test]
    async fn lists_by_state() {
        let (store, source) = seeded().await;
        let kept = store.add_item(&post_item(source)).await.unwrap();
        store
            .add_item(&media_item(source, State::Holding))
            .await
            .unwrap();

        let found = store.items_in_state(StateName::Kept).await.unwrap();
        assert_eq!(found.iter().map(|i| i.id).collect::<Vec<_>>(), [kept]);
        assert_eq!(
            store
                .items_in_state(StateName::Holding)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .items_in_state(StateName::Error)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 読んだときの状態が動いていたら書かない。
    #[tokio::test]
    async fn applying_an_event_is_compare_and_swap() {
        let (store, source) = seeded().await;
        let id = store
            .add_item(&media_item(source, State::Holding))
            .await
            .unwrap();
        let now = at("2026-03-08T09:00:00+09:00");

        // 別プロセスが先に holding -> kept へ動かした、という体。
        assert_eq!(
            store
                .apply(id, &State::Holding, &Event::SourceGone, now)
                .await
                .unwrap(),
            Applied::Written(State::Kept)
        );

        // こちらは holding のつもりで期限切れを適用しようとする。
        // 遷移としては合法（holding -> discarded）だが、もう holding ではない。
        let confirmed = crate::liveness::Presence::Present.confirmed().unwrap();
        assert_eq!(
            store
                .apply(id, &State::Holding, &Event::HeldToDeadline(confirmed), now)
                .await
                .unwrap(),
            Applied::Superseded,
            "先に動かされていたら書かない"
        );

        assert_eq!(store.item(id).await.unwrap().unwrap().state, State::Kept);
    }

    /// 図に無い遷移は DB まで届かない。次の状態を直接渡す口が無いため。
    #[tokio::test]
    async fn an_illegal_transition_never_reaches_the_database() {
        let (store, source) = seeded().await;
        let id = store
            .add_item(&media_item(source, State::Holding))
            .await
            .unwrap();

        // #4 の「holding では release できない」。
        let result = store
            .apply(
                id,
                &State::Holding,
                &Event::Released { reference: None },
                at("2026-03-02T10:00:00+09:00"),
            )
            .await;

        assert!(matches!(result, Err(StoreError::IllegalTransition(_))));
        assert_eq!(store.item(id).await.unwrap().unwrap().state, State::Holding);
    }

    #[tokio::test]
    async fn releasing_carries_the_reference() {
        let (store, source) = seeded().await;
        let id = store
            .add_item(&media_item(source, State::Kept))
            .await
            .unwrap();

        let reference = ReleaseReference::new("Attachments/○○.mp4");
        let applied = store
            .apply(
                id,
                &State::Kept,
                &Event::Released {
                    reference: Some(reference.clone()),
                },
                at("2026-03-02T10:00:00+09:00"),
            )
            .await
            .unwrap();

        let expected = State::Released {
            reference: Some(reference),
        };
        assert_eq!(applied, Applied::Written(expected.clone()));
        assert_eq!(store.item(id).await.unwrap().unwrap().state, expected);
    }

    /// 同じ配信元を二度登録できない。できてしまうと、二個目は item.url の
    /// UNIQUE に阻まれて検知が毎回空振りする。
    #[tokio::test]
    async fn the_same_source_cannot_be_registered_twice() {
        let (store, source_id) = seeded().await;
        let source = store.source(source_id).await.unwrap().unwrap();
        let person = store.add_person("△△").await.unwrap();

        let again = store
            .add_source(&NewSource {
                person_id: person,
                url: source.url.clone(),
                enabled: true,
                created_at: at("2026-02-01T00:00:00+09:00"),
                hold_days: None,
                discover_interval_minutes: NonZeroU32::new(5).unwrap(),
            })
            .await;

        assert!(again.is_err(), "別の持ち主でも同じ配信元は二度入らない");
    }

    /// #1 の「持ち主のいない `Source` は作れない」と、削除の連鎖。
    #[tokio::test]
    async fn deleting_a_person_takes_its_sources_and_items() {
        let (store, source) = seeded().await;
        store.add_item(&post_item(source)).await.unwrap();

        sqlx::query("DELETE FROM person")
            .execute(store.pool())
            .await
            .unwrap();

        assert!(store.source(source).await.unwrap().is_none());
        assert!(
            store
                .items_in_state(StateName::Kept)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_source_without_an_owner_is_refused() {
        let store = Store::open_in_memory().await.unwrap();
        let orphan = store
            .add_source(&NewSource {
                person_id: PersonId::new(999),
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled: true,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days: None,
                discover_interval_minutes: NonZeroU32::new(15).unwrap(),
            })
            .await;

        assert!(orphan.is_err(), "外部キーが効くこと");
    }

    #[tokio::test]
    async fn round_trips_a_source() {
        let (store, source_id) = seeded().await;
        let source = store.source(source_id).await.unwrap().unwrap();

        assert_eq!(source.hold_days, NonZeroU32::new(7));
        assert_eq!(source.discover_interval_minutes.get(), 15);
        assert!(source.enabled);
        assert_eq!(source.hold_policy(), crate::state::HoldPolicy::Hold);
    }

    /// 崩し方ごとに、どのエラーになるはずかを見る述語。
    type Expected = fn(&RowError) -> bool;

    /// 壊れた行は黙って通さない。#1 の「`NULL` を許すのはこの列だけ」が
    /// 文書ではなく parse として効いていること。
    #[tokio::test]
    async fn refuses_rows_that_break_the_null_rules() {
        // (壊し方, 期待するエラー) — 種別と状態の組み合わせを1つずつ崩す。
        let cases: Vec<(&str, Expected)> = vec![
            ("UPDATE item SET title = NULL", |e| {
                matches!(
                    e,
                    RowError::MissingColumn {
                        column: "title",
                        ..
                    }
                )
            }),
            (
                "UPDATE item SET body = 'これは Media には無いはず'",
                |e| matches!(e, RowError::UnexpectedColumn { column: "body", .. }),
            ),
            (
                "UPDATE item SET in_reply_to_url = 'https://x.com/i/status/20'",
                |e| {
                    matches!(
                        e,
                        RowError::UnexpectedColumn {
                            column: "in_reply_to_url",
                            ..
                        }
                    )
                },
            ),
            ("UPDATE item SET release_reference = 'どこか'", |e| {
                matches!(e, RowError::ReferenceWithoutRelease { .. })
            }),
            ("UPDATE item SET content_type = 'youtube_livestream'", |e| {
                matches!(e, RowError::UnknownContentType { .. })
            }),
            ("UPDATE item SET state = 'gone'", |e| {
                matches!(e, RowError::UnknownState { .. })
            }),
            (
                "UPDATE item SET url = 'https://youtu.be/dQw4w9WgXcQ'",
                |e| matches!(e, RowError::UnnormalizedUrl { column: "url", .. }),
            ),
            ("UPDATE item SET published_at = 'きのう'", |e| {
                matches!(
                    e,
                    RowError::BadTimestamp {
                        column: "published_at",
                        ..
                    }
                )
            }),
        ];

        for (corruption, expected) in cases {
            let (store, source) = seeded().await;
            let id = store
                .add_item(&media_item(source, State::Holding))
                .await
                .unwrap();

            sqlx::query(corruption).execute(store.pool()).await.unwrap();

            match store.item(id).await {
                Err(StoreError::Row(e)) => {
                    assert!(expected(&e), "{corruption} で想定外のエラー: {e}");
                }
                other => panic!("{corruption} が通ってしまった: {other:?}"),
            }
        }
    }

    /// `Post` 側の欠けも同じように捕まえる。
    #[tokio::test]
    async fn refuses_a_post_without_a_body() {
        let (store, source) = seeded().await;
        let id = store.add_item(&post_item(source)).await.unwrap();

        sqlx::query("UPDATE item SET body = NULL")
            .execute(store.pool())
            .await
            .unwrap();

        assert!(matches!(
            store.item(id).await,
            Err(StoreError::Row(RowError::MissingColumn {
                column: "body",
                ..
            }))
        ));
    }

    /// 古いバイナリが新しい DB を開いたら止まること。
    ///
    /// 配布が `brew upgrade` なので降格でこれが起きる。sqlx が既定で
    /// `VersionMissing` にするので、こちらで手当てを書かなくてよい（#7）。
    #[tokio::test]
    async fn refuses_a_database_from_a_newer_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("escrow.db");

        let store = Store::open(&path).await.unwrap();
        // このバイナリが知らない移行が当たっている、という体。
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, installed_on, success, checksum, execution_time) \
             VALUES (999, 'from the future', datetime('now'), 1, X'00', 0)",
        )
        .execute(store.pool())
        .await
        .unwrap();
        drop(store);

        match Store::open(&path).await {
            Err(StoreError::Migrate(sqlx::migrate::MigrateError::VersionMissing(999))) => {}
            other => panic!("降格を止めなかった: {other:?}"),
        }
    }

    #[tokio::test]
    async fn round_trips_excludes() {
        let (store, source) = seeded().await;
        store
            .add_exclude(Some(source), ContentType::XSpace, true)
            .await
            .unwrap();
        store
            .add_exclude(None, ContentType::YoutubeShorts, true)
            .await
            .unwrap();

        let excludes = store.excludes().await.unwrap();
        assert_eq!(excludes.len(), 2);
        assert_eq!(excludes[0].source_id, Some(source));
        assert_eq!(excludes[1].source_id, None, "共通条件は source_id が空");
    }
}

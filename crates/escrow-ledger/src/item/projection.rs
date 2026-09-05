//! `item` テーブル。#1 の erDiagram をそのまま写した投影（#15）。
//!
//! 列も索引も事象ログを入れる前と同じなので、**読み出しのクエリと性能は変わらない**。
//! 変わったのは書き込みだけ。
//!
//! 読み出しは必ず `seq` を連れて返す。次の事象を書くときの前提になるので、
//! 根拠を持たずに書く経路を作らせない。

use escrow_domain::content::{Content, ContentType};
use escrow_domain::item::{Item, ItemId};
use escrow_domain::source::SourceId;
use escrow_domain::state::{Hold, ReleaseReference, State, StateName};
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::NormalizedUrl;

use super::Projected;
use crate::{Ledger, LedgerError, RowError, Seq, content_type_of, normalized, timestamp};

/// `item` の1行と、その姿を決めた最後の事象の番号。ここから先はドメイン型。
struct Row {
    id: i64,
    source_id: i64,
    url: String,
    content_type: String,
    published_at: String,
    scheduled_start_at: Option<String>,
    hold_until: Option<String>,
    state: String,
    state_since: String,
    title: Option<String>,
    body: Option<String>,
    in_reply_to_url: Option<String>,
    quoted_url: Option<String>,
    release_reference: Option<String>,
    seq: i64,
}

impl Ledger {
    /// `id` で1件読む。
    pub async fn item(&self, id: ItemId) -> Result<Option<Projected>, LedgerError> {
        let key = i64::from(id);
        let row = sqlx::query_as!(
            Row,
            r#"SELECT i.id AS "id!", i.source_id, i.url, i.content_type, i.published_at,
                      i.scheduled_start_at, i.hold_until, i.state, i.state_since,
                      i.title, i.body, i.in_reply_to_url, i.quoted_url, i.release_reference,
                      (SELECT MAX(e.seq) FROM item_event e WHERE e.item_id = i.id) AS "seq!: i64"
               FROM item i WHERE i.id = ?"#,
            key
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(Projected::try_from).transpose().map_err(Into::into)
    }

    /// 正規化した URL で1件読む。項目の自然キー（#1）。
    pub async fn item_by_url(&self, url: &NormalizedUrl) -> Result<Option<Projected>, LedgerError> {
        let key = url.as_str();
        let row = sqlx::query_as!(
            Row,
            r#"SELECT i.id AS "id!", i.source_id, i.url, i.content_type, i.published_at,
                      i.scheduled_start_at, i.hold_until, i.state, i.state_since,
                      i.title, i.body, i.in_reply_to_url, i.quoted_url, i.release_reference,
                      (SELECT MAX(e.seq) FROM item_event e WHERE e.item_id = i.id) AS "seq!: i64"
               FROM item i WHERE i.url = ?"#,
            key
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(Projected::try_from).transpose().map_err(Into::into)
    }

    /// 状態で絞って読む。エンジンはこれで拾ったものを、対応するスライスへ渡す（#15）。
    pub async fn items_in_state(&self, state: StateName) -> Result<Vec<Projected>, LedgerError> {
        let key = state.as_str();
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT i.id AS "id!", i.source_id, i.url, i.content_type, i.published_at,
                      i.scheduled_start_at, i.hold_until, i.state, i.state_since,
                      i.title, i.body, i.in_reply_to_url, i.quoted_url, i.release_reference,
                      (SELECT MAX(e.seq) FROM item_event e WHERE e.item_id = i.id) AS "seq!: i64"
               FROM item i WHERE i.state = ? ORDER BY i.id"#,
            key
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(Projected::try_from)
            .collect::<Result<_, _>>()
            .map_err(Into::into)
    }
}

impl TryFrom<Row> for Projected {
    type Error = RowError;

    fn try_from(row: Row) -> Result<Self, Self::Error> {
        let id = row.id;
        let content_type = content_type_of(id, &row.content_type)?;

        let content = content_of(
            id,
            content_type,
            row.title.as_deref(),
            row.body.as_deref(),
            row.in_reply_to_url.as_deref(),
            row.quoted_url.as_deref(),
        )?;

        let hold_until = row
            .hold_until
            .as_deref()
            .map(|text| timestamp(id, "hold_until", text))
            .transpose()?;

        let state = state_of(
            id,
            &row.state,
            hold_until,
            row.release_reference.map(ReleaseReference::new),
        )?;

        let seq = seq_of(id, row.seq)?;

        Ok(Self {
            item: Item {
                id: ItemId::new(id),
                source_id: SourceId::new(row.source_id),
                url: normalized(id, "url", &row.url)?,
                published_at: timestamp(id, "published_at", &row.published_at)?,
                scheduled_start_at: row
                    .scheduled_start_at
                    .as_deref()
                    .map(|text| timestamp(id, "scheduled_start_at", text))
                    .transpose()?,
                state,
                state_since: timestamp(id, "state_since", &row.state_since)?,
                content,
            },
            seq,
        })
    }
}

/// 種別が `Media` 側か `Post` 側かで、要る列と、あってはいけない列が決まる（#1）。
pub(super) fn content_of(
    id: i64,
    content_type: ContentType,
    title: Option<&str>,
    body: Option<&str>,
    in_reply_to_url: Option<&str>,
    quoted_url: Option<&str>,
) -> Result<Content, RowError> {
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
            let title = title.ok_or_else(|| missing("title"))?.to_owned();
            // Media は body も繋がりの URL も持たない（#1）。
            if body.is_some() {
                return Err(unexpected("body"));
            }
            if in_reply_to_url.is_some() {
                return Err(unexpected("in_reply_to_url"));
            }
            if quoted_url.is_some() {
                return Err(unexpected("quoted_url"));
            }
            Ok(Content::Media { media_type, title })
        }
        None => {
            let body = body.ok_or_else(|| missing("body"))?.to_owned();
            if title.is_some() {
                return Err(unexpected("title"));
            }
            Ok(Content::Post {
                body,
                in_reply_to: in_reply_to_url
                    .map(|u| normalized(id, "in_reply_to_url", u))
                    .transpose()?,
                quoted: quoted_url
                    .map(|u| normalized(id, "quoted_url", u))
                    .transpose()?,
            })
        }
    }
}

/// 状態は3列が揃って初めて決まる — 名前・預かりの期限・引き渡し先の参照（#1）。
///
/// 1列だけ見て組み立てると、期限を持たない `holding` や、参照を落とした `released` が
/// 静かに出来上がる。ここが**行を丸ごと見る parse** の実体。
pub(super) fn state_of(
    id: i64,
    name: &str,
    hold_until: Option<Timestamp>,
    reference: Option<ReleaseReference>,
) -> Result<State, RowError> {
    let name: StateName = name.parse().map_err(|_| RowError::UnknownState {
        id,
        value: name.to_owned(),
    })?;

    let carries_a_deadline = matches!(name, StateName::Transcribing | StateName::Holding);
    if !carries_a_deadline && hold_until.is_some() {
        return Err(RowError::ColumnWithoutState {
            id,
            state: name,
            column: "hold_until",
        });
    }
    if name != StateName::Released && reference.is_some() {
        return Err(RowError::ColumnWithoutState {
            id,
            state: name,
            column: "release_reference",
        });
    }

    Ok(match name {
        StateName::Waiting => State::Waiting,
        StateName::Acquiring => State::Acquiring,
        StateName::Transcribing => State::Transcribing {
            hold: match hold_until {
                Some(until) => Hold::Until(until),
                None => Hold::None,
            },
        },
        StateName::Holding => State::Holding {
            until: hold_until.ok_or(RowError::StateMissingColumn {
                id,
                state: name,
                column: "hold_until",
            })?,
        },
        StateName::Kept => State::Kept,
        StateName::Discarded => State::Discarded,
        StateName::Released => State::Released { reference },
        StateName::Deleted => State::Deleted,
        StateName::Error => State::Error,
    })
}

pub(super) fn seq_of(id: i64, value: i64) -> Result<Seq, RowError> {
    u32::try_from(value)
        .ok()
        .and_then(std::num::NonZeroU32::new)
        .map(Seq::from_stored)
        .ok_or(RowError::OutOfRange {
            table: "item_event",
            id,
            column: "seq",
            value,
        })
}

/// ドメイン型から、投影の「`NULL` を許す」列を取り出したもの。
///
/// 書く側と読む側がここで対になる。[`State`] が伴う値は状態自身から取るので、
/// 状態と列が食い違う書き方ができない。
pub(crate) struct Columns {
    pub title: Option<String>,
    pub body: Option<String>,
    pub in_reply_to_url: Option<String>,
    pub quoted_url: Option<String>,
    pub hold_until: Option<String>,
    pub release_reference: Option<String>,
}

impl Columns {
    pub(crate) fn of(content: &Content, state: &State) -> Self {
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
            hold_until: state.hold_until().map(Timestamp::to_text),
            release_reference: state.release_reference().map(|r| r.as_str().to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{a_holding_item, a_post, at, seeded};
    use crate::{LedgerError, RowError};

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
                matches!(
                    e,
                    RowError::ColumnWithoutState {
                        column: "release_reference",
                        ..
                    }
                )
            }),
            // 預かりを通らない状態に期限が入っていたら撥ねる（#1）。
            ("UPDATE item SET state = 'kept'", |e| {
                matches!(
                    e,
                    RowError::ColumnWithoutState {
                        column: "hold_until",
                        ..
                    }
                )
            }),
            // 逆に holding から期限を抜いても撥ねる。期限のない預かりは無い。
            ("UPDATE item SET hold_until = NULL", |e| {
                matches!(
                    e,
                    RowError::StateMissingColumn {
                        column: "hold_until",
                        ..
                    }
                )
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
            ("UPDATE item SET scheduled_start_at = 'あした'", |e| {
                matches!(
                    e,
                    RowError::BadTimestamp {
                        column: "scheduled_start_at",
                        ..
                    }
                )
            }),
        ];

        for (corruption, expected) in cases {
            let (ledger, source) = seeded().await;
            let id = a_holding_item(&ledger, source).await;

            sqlx::query(corruption).execute(&ledger.pool).await.unwrap();

            match ledger.item(id).await {
                Err(LedgerError::Row(e)) => {
                    assert!(expected(&e), "{corruption} で想定外のエラー: {e}");
                }
                other => panic!("{corruption} が通ってしまった: {other:?}"),
            }
        }
    }

    /// `Post` 側の欠けも同じように捕まえる。
    #[tokio::test]
    async fn refuses_a_post_without_a_body() {
        let (ledger, source) = seeded().await;
        let id = ledger
            .discover(&a_post(source), at("2026-03-01T12:01:00+09:00"))
            .await
            .unwrap();

        sqlx::query("UPDATE item SET body = NULL")
            .execute(&ledger.pool)
            .await
            .unwrap();

        assert!(matches!(
            ledger.item(id).await,
            Err(LedgerError::Row(RowError::MissingColumn {
                column: "body",
                ..
            }))
        ));
    }

    /// 繋がりの URL も往復すること（#1）。
    #[tokio::test]
    async fn round_trips_a_post_with_its_links() {
        let (ledger, source) = seeded().await;
        let post = a_post(source);
        let id = ledger
            .discover(&post, at("2026-03-01T12:01:00+09:00"))
            .await
            .unwrap();

        let read = ledger.item(id).await.unwrap().unwrap();
        assert_eq!(read.item.content, post.content);
    }

    /// 入口が違っても同じ正規形になるので、同じ行に着く（#1）。
    #[tokio::test]
    async fn finds_an_item_by_its_natural_key() {
        let (ledger, source) = seeded().await;
        let id = a_holding_item(&ledger, source).await;

        let found = ledger
            .item_by_url(&crate::testing::item_url(
                "https://youtu.be/dQw4w9WgXcQ?si=xyz",
            ))
            .await
            .unwrap()
            .expect("正規化して引ける");
        assert_eq!(found.item.id, id);
    }
}

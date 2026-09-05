//! ログを読み、`state::next` で畳んで現在を作る（#15）。
//!
//! 投影を経由しないので、**投影が正しいかを確かめる側**にもなる。畳んだ結果と
//! 投影が一致することが、この2つを同じトランザクションで書いている証拠になる。

use escrow_domain::item::{Discovered, Item, ItemId};
use escrow_domain::liveness::{Presence, PresenceConfirmed};
use escrow_domain::source::SourceId;
use escrow_domain::state::{
    Event, EventKind, FailureReason, Hold, MediaPresence, ReleaseReference, TranscriptNeed,
};

use super::projection::{content_of, seq_of};
use super::{Log, Recorded};
use crate::{Ledger, LedgerError, RowError, Seq, content_type_of, normalized, timestamp};

/// `item_event` の1行そのまま。
pub(crate) struct EventRow {
    pub item_id: i64,
    pub source_id: i64,
    pub seq: i64,
    pub kind: String,
    pub occurred_at: String,
    pub url: Option<String>,
    pub content_type: Option<String>,
    pub published_at: Option<String>,
    pub scheduled_start_at: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub in_reply_to_url: Option<String>,
    pub quoted_url: Option<String>,
    pub media_present: Option<bool>,
    pub transcript_needed: Option<bool>,
    pub hold_until: Option<String>,
    pub release_reference: Option<String>,
    pub failure_reason: Option<String>,
}

impl Ledger {
    /// 1つの項目のログ全体。リトライ回数のように、畳んだ結果では答えられないものを
    /// 訊く側が使う（#1）。
    pub async fn log(&self, id: ItemId) -> Result<Option<Log>, LedgerError> {
        let key = i64::from(id);
        let rows = sqlx::query_as!(
            EventRow,
            r#"SELECT item_id AS "item_id!", source_id, seq, kind, occurred_at,
                      url, content_type, published_at, scheduled_start_at,
                      title, body, in_reply_to_url, quoted_url,
                      media_present AS "media_present: bool",
                      transcript_needed AS "transcript_needed: bool",
                      hold_until, release_reference, failure_reason
               FROM item_event WHERE item_id = ? ORDER BY seq"#,
            key
        )
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(log_of(rows)?))
    }

    /// ログを畳んだ、いまの姿。投影を読まない。
    pub async fn replay(&self, id: ItemId) -> Result<Option<Item>, LedgerError> {
        let Some(log) = self.log(id).await? else {
            return Ok(None);
        };
        Ok(Some(log.replay()?))
    }
}

/// `seq` の順に並んだ、1つの項目ぶんの行からログを組む。
///
/// **先頭が `discovered` で、番号が 1 から途切れずに続くこと**をここで確かめる。
/// 確かめ終えた形が [`Log`] なので、畳む側はもう何も確かめなくてよい。
pub(crate) fn log_of(rows: Vec<EventRow>) -> Result<Log, RowError> {
    let mut rows = rows.into_iter();
    let head = rows.next().expect("呼ぶ側が空でないことを確かめている");
    let id = head.item_id;

    if kind_of(id, &head.kind)? != EventKind::Discovered || head.seq != 1 {
        return Err(RowError::LogDoesNotBegin { id });
    }

    let discovered_at = timestamp(id, "occurred_at", &head.occurred_at)?;
    let discovered = discovered_of(&head)?;

    let mut rest = Vec::new();
    let mut expected = Seq::FIRST;
    for row in rows {
        expected = expected.next();
        if row.seq != i64::from(expected.get()) {
            return Err(RowError::LogOutOfOrder {
                id,
                expected: expected.get(),
                actual: row.seq,
            });
        }
        rest.push(Recorded {
            seq: seq_of(id, row.seq)?,
            occurred_at: timestamp(id, "occurred_at", &row.occurred_at)?,
            event: event_of(&row)?,
        });
    }

    Ok(Log {
        id: ItemId::new(id),
        discovered,
        discovered_at,
        rest,
    })
}

fn kind_of(id: i64, value: &str) -> Result<EventKind, RowError> {
    value.parse().map_err(|_| RowError::UnknownEventKind {
        id,
        value: value.to_owned(),
    })
}

/// 誕生の行から、項目の中身を組み直す。投影を捨てても戻せる根拠がここ（#1）。
fn discovered_of(row: &EventRow) -> Result<Discovered, RowError> {
    let id = row.item_id;
    let missing = |column| RowError::EventMissingColumn {
        id,
        kind: EventKind::Discovered,
        column,
    };

    let url = row.url.as_deref().ok_or_else(|| missing("url"))?;
    let content_type = content_type_of(
        id,
        row.content_type
            .as_deref()
            .ok_or_else(|| missing("content_type"))?,
    )?;
    let published_at = row
        .published_at
        .as_deref()
        .ok_or_else(|| missing("published_at"))?;
    let media_present = row.media_present.ok_or_else(|| missing("media_present"))?;

    Ok(Discovered {
        source_id: SourceId::new(row.source_id),
        url: normalized(id, "url", url)?,
        published_at: timestamp(id, "published_at", published_at)?,
        scheduled_start_at: row
            .scheduled_start_at
            .as_deref()
            .map(|text| timestamp(id, "scheduled_start_at", text))
            .transpose()?,
        content: content_of(
            id,
            content_type,
            row.title.as_deref(),
            row.body.as_deref(),
            row.in_reply_to_url.as_deref(),
            row.quoted_url.as_deref(),
        )?,
        media: if media_present {
            MediaPresence::Present
        } else {
            MediaPresence::Absent
        },
    })
}

/// 判別子で全域に分岐する。受け皿を置かないので、事象を足すとここが落ちる。
fn event_of(row: &EventRow) -> Result<Event, RowError> {
    let id = row.item_id;
    let kind = kind_of(id, &row.kind)?;
    let missing = |column| RowError::EventMissingColumn { id, kind, column };

    Ok(match kind {
        // 先頭にしか来ない。log_of がそれを確かめている。
        EventKind::Discovered => return Err(RowError::LogDoesNotBegin { id }),
        EventKind::AcquisitionStarted => Event::AcquisitionStarted,
        EventKind::Acquired => Event::Acquired {
            transcript: match row
                .transcript_needed
                .ok_or_else(|| missing("transcript_needed"))?
            {
                true => TranscriptNeed::Needed,
                false => TranscriptNeed::NotNeeded,
            },
            hold: match row.hold_until.as_deref() {
                Some(text) => Hold::Until(timestamp(id, "hold_until", text)?),
                None => Hold::None,
            },
        },
        EventKind::Transcribed => Event::Transcribed,
        EventKind::SourceGone => Event::SourceGone,
        EventKind::PresenceConfirmed => Event::PresenceConfirmed(confirmed()),
        EventKind::HeldToDeadline => Event::HeldToDeadline(confirmed()),
        EventKind::Released => Event::Released {
            reference: row.release_reference.clone().map(ReleaseReference::new),
        },
        EventKind::Deleted => Event::Deleted,
        EventKind::AttemptFailed => Event::AttemptFailed {
            reason: FailureReason::new(
                row.failure_reason
                    .clone()
                    .ok_or_else(|| missing("failure_reason"))?,
            ),
        },
        EventKind::RetriesExhausted => Event::RetriesExhausted,
        EventKind::ReacquisitionRequested => Event::ReacquisitionRequested,
    })
}

/// 保存された確認の行を、証として読み直す。
///
/// 証は「確かめた」という事実の型で、行が在ること自体がその事実。書いた時点で
/// [`Presence::Present`] が取れていなければ行は無い（#1 の「沈黙は記録されない」）。
fn confirmed() -> PresenceConfirmed {
    Presence::Present
        .confirmed()
        .expect("Present からは必ず証が取れる")
}

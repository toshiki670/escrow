//! 事象を書く2つの道 — 誕生と追記（#15）。
//!
//! どちらも1つのトランザクションで、**事象を書いてから投影へ反映する**。投影の値は
//! 決定ではなく決定の写し。
//!
//! 追記は `(item_id, seq)` の UNIQUE が競合を弾く。

use escrow_domain::item::{Discovered, ItemId};
use escrow_domain::state::{Event, Hold, MediaPresence, TranscriptNeed, next};
use escrow_domain::timestamp::Timestamp;

use super::projection::{Columns, state_of};
use crate::{Ledger, LedgerError, Seq, timestamp};

impl Ledger {
    /// 項目を起票する。
    ///
    /// ログの先頭に `discovered` を1つ書き、そこから投影の行を作る。`url` の
    /// `UNIQUE` は投影が持っているが、両方を同じトランザクションで書くので、
    /// 同じ URL の2件目はログにも入らない（#1）。
    pub async fn discover(
        &self,
        discovered: &Discovered,
        at: Timestamp,
    ) -> Result<ItemId, LedgerError> {
        let state = discovered.initial_state();
        let columns = Columns::of(&discovered.content, &state);

        let source_id = i64::from(discovered.source_id);
        let url = discovered.url.as_str();
        let content_type = discovered.content_type().as_str();
        let published_at = discovered.published_at.to_text();
        let scheduled_start_at = discovered.scheduled_start_at.map(Timestamp::to_text);
        let media_present = i64::from(matches!(discovered.media, MediaPresence::Present));
        let occurred_at = at.to_text();
        let state_name = state.as_str();
        let seq = i64::from(Seq::FIRST.get());

        let mut tx = self.pool.begin().await?;

        // 同一性はログが持つ。投影の rowid から採ると、真実の側が捨てられる側の
        // 採番に依存することになる。
        let id =
            sqlx::query!(r#"SELECT COALESCE(MAX(item_id), 0) + 1 AS "id!: i64" FROM item_event"#)
                .fetch_one(&mut *tx)
                .await?
                .id;

        sqlx::query!(
            "INSERT INTO item_event (item_id, source_id, seq, kind, occurred_at, url, \
             content_type, published_at, scheduled_start_at, title, body, in_reply_to_url, \
             quoted_url, media_present) \
             VALUES (?, ?, ?, 'discovered', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            source_id,
            seq,
            occurred_at,
            url,
            content_type,
            published_at,
            scheduled_start_at,
            columns.title,
            columns.body,
            columns.in_reply_to_url,
            columns.quoted_url,
            media_present,
        )
        .execute(&mut *tx)
        .await
        .map_err(LedgerError::from_append)?;

        sqlx::query!(
            "INSERT INTO item (id, source_id, url, content_type, published_at, state, \
             state_since, scheduled_start_at, hold_until, title, body, in_reply_to_url, \
             quoted_url, release_reference) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            source_id,
            url,
            content_type,
            published_at,
            state_name,
            occurred_at,
            scheduled_start_at,
            columns.hold_until,
            columns.title,
            columns.body,
            columns.in_reply_to_url,
            columns.quoted_url,
            columns.release_reference,
        )
        .execute(&mut *tx)
        .await
        .map_err(LedgerError::from_append)?;

        tx.commit().await?;

        Ok(ItemId::new(id))
    }

    /// 事象を1つ追記し、同じトランザクションで投影へ反映する。
    ///
    /// `after` は**その決定の前提**。読んだときの `seq` をそのまま渡す。ずれていれば
    /// 誰かが先に動かしているので、読み直して決め直す（#7）。
    ///
    /// `state_since` は状態が変わったときだけ動く。生存確認や1回の失敗では動かない。
    pub async fn append(
        &self,
        id: ItemId,
        after: Seq,
        event: &Event,
        at: Timestamp,
    ) -> Result<Seq, LedgerError> {
        let key = i64::from(id);
        let mut tx = self.pool.begin().await?;

        let row = sqlx::query!(
            r#"SELECT i.state, i.state_since, i.hold_until, i.release_reference,
                      (SELECT MAX(e.seq) FROM item_event e WHERE e.item_id = i.id) AS "seq!: i64"
               FROM item i WHERE i.id = ?"#,
            key
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            return Err(LedgerError::NoSuchItem(id));
        };

        // 番号がずれていれば、この決定は古い姿を見て下されている。
        // 同時に走る2つのうち片方は、この先の UNIQUE でも弾かれる。
        if u32::try_from(row.seq).is_ok_and(|seq| seq != after.get()) {
            return Err(LedgerError::Superseded);
        }

        let hold_until = row
            .hold_until
            .as_deref()
            .map(|text| timestamp(key, "hold_until", text))
            .transpose()?;
        let state = state_of(
            key,
            &row.state,
            hold_until,
            row.release_reference
                .map(escrow_domain::state::ReleaseReference::new),
        )?;

        let moved = next(&state, event)?;
        let state_since = if moved == state {
            timestamp(key, "state_since", &row.state_since)?
        } else {
            at
        };

        let payload = Payload::of(event);
        let seq = after.next();
        let seq_column = i64::from(seq.get());
        let kind = event.kind().as_str();
        let occurred_at = at.to_text();

        sqlx::query!(
            "INSERT INTO item_event (item_id, source_id, seq, kind, occurred_at, \
             transcript_needed, hold_until, release_reference, failure_reason) \
             SELECT ?, i.source_id, ?, ?, ?, ?, ?, ?, ? FROM item i WHERE i.id = ?",
            key,
            seq_column,
            kind,
            occurred_at,
            payload.transcript_needed,
            payload.hold_until,
            payload.release_reference,
            payload.failure_reason,
            key,
        )
        .execute(&mut *tx)
        .await
        .map_err(LedgerError::from_append)?;

        let state_name = moved.as_str();
        let state_since = state_since.to_text();
        let projected_hold_until = moved.hold_until().map(Timestamp::to_text);
        let projected_reference = moved.release_reference().map(|r| r.as_str().to_owned());

        sqlx::query!(
            "UPDATE item SET state = ?, state_since = ?, hold_until = ?, release_reference = ? \
             WHERE id = ?",
            state_name,
            state_since,
            projected_hold_until,
            projected_reference,
            key,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(seq)
    }
}

/// 事象ごとの平らな列（#1）。運ばない事象では全部 `NULL`。
struct Payload {
    transcript_needed: Option<i64>,
    hold_until: Option<String>,
    release_reference: Option<String>,
    failure_reason: Option<String>,
}

impl Payload {
    fn of(event: &Event) -> Self {
        let empty = Self {
            transcript_needed: None,
            hold_until: None,
            release_reference: None,
            failure_reason: None,
        };

        match event {
            Event::Acquired { transcript, hold } => Self {
                transcript_needed: Some(i64::from(matches!(transcript, TranscriptNeed::Needed))),
                hold_until: match hold {
                    Hold::Until(until) => Some(until.to_text()),
                    Hold::None => None,
                },
                ..empty
            },
            Event::Released { reference } => Self {
                release_reference: reference.as_ref().map(|r| r.as_str().to_owned()),
                ..empty
            },
            Event::AttemptFailed { reason } => Self {
                failure_reason: Some(reason.as_str().to_owned()),
                ..empty
            },
            Event::AcquisitionStarted
            | Event::Transcribed
            | Event::SourceGone
            | Event::PresenceConfirmed(_)
            | Event::HeldToDeadline(_)
            | Event::Deleted
            | Event::RetriesExhausted
            | Event::ReacquisitionRequested => empty,
        }
    }
}

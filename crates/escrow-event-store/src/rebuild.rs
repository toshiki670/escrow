//! リードモデルを捨てて作り直す（#15）。
//!
//! リードモデルのスキーマを変えたいときは、移行ではなくこれを走らせる。

use escrow_domain::timestamp::Timestamp;
use sqlx::Executor;

use crate::item::{Columns, EventRow, log_of};
use crate::{EventStore, EventStoreError, READ_MODEL, WRITE};

impl EventStore {
    /// リードモデルを DROP して作り直し、ログから埋め直す。
    ///
    /// 戻すのは作り直した項目の数。起動時にリードモデルを作るのと同じ DDL を流す。
    pub async fn rebuild(&self) -> Result<u64, EventStoreError> {
        let mut tx = self.pool.begin_with(WRITE).await?;

        tx.execute("DROP TABLE IF EXISTS item").await?;
        tx.execute(READ_MODEL).await?;

        let rows = sqlx::query_as!(
            EventRow,
            r#"SELECT item_id AS "item_id!", source_id, seq, kind, occurred_at,
                      url, content_type, published_at, scheduled_start_at,
                      title, body, in_reply_to_url, quoted_url,
                      media_present AS "media_present: bool",
                      transcript_needed AS "transcript_needed: bool",
                      hold_until, release_reference, failure_reason
               FROM item_event ORDER BY item_id, seq"#
        )
        .fetch_all(&mut *tx)
        .await?;

        let mut rebuilt = 0;
        for group in group_by_item(rows) {
            let item = log_of(group)?.replay()?;
            let columns = Columns::of(&item.content, &item.state);

            let id = i64::from(item.id);
            let source_id = i64::from(item.source_id);
            let url = item.url.as_str();
            let content_type = item.content_type().as_str();
            let published_at = item.published_at.to_text();
            let state = item.state.as_str();
            let state_since = item.state_since.to_text();
            let scheduled_start_at = item.scheduled_start_at.map(Timestamp::to_text);

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
                state,
                state_since,
                scheduled_start_at,
                columns.hold_until,
                columns.title,
                columns.body,
                columns.in_reply_to_url,
                columns.quoted_url,
                columns.release_reference,
            )
            .execute(&mut *tx)
            .await?;

            rebuilt += 1;
        }

        tx.commit().await?;

        Ok(rebuilt)
    }
}

/// `item_id` の順に並んだ行を、項目ごとにまとめる。
fn group_by_item(rows: Vec<EventRow>) -> Vec<Vec<EventRow>> {
    let mut groups: Vec<Vec<EventRow>> = Vec::new();

    for row in rows {
        match groups.last_mut() {
            Some(group) if group[0].item_id == row.item_id => group.push(row),
            _ => groups.push(vec![row]),
        }
    }

    groups
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use escrow_domain::content::{Content, MediaType};
    use escrow_domain::item::Discovered;
    use escrow_domain::source::Monitoring;
    use escrow_domain::state::{Event, Hold, MediaPresence, State, TranscriptNeed};
    use escrow_domain::url;

    use crate::testing::at;
    use crate::{EventStore, NewSource, Seq};

    /// リードモデルを壊してから作り直し、元に戻ることを確かめる。
    ///
    /// リードモデルを書き換えられるのは crate の中だけなので、**壊す側もここにしか書けない**。
    /// それ自体が「リードモデルは追記からしか動かない」の裏付けになっている。
    #[tokio::test]
    async fn a_broken_read_model_is_restored_from_the_log() {
        let store = EventStore::open_in_memory().await.unwrap();
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
                hold_days: Some(NonZeroU32::new(7).unwrap()),
                priority: NonZeroU32::MIN,
                monitoring: Monitoring::Continuous,
            })
            .await
            .unwrap();

        let deadline = at("2026-03-09T00:30:00+09:00");
        let mut ids = Vec::new();
        for video in ["dQw4w9WgXcQ", "bLKBe3uMMRI"] {
            let id = store
                .discover(
                    &Discovered {
                        source_id: source,
                        url: url::normalize_item(&format!(
                            "https://www.youtube.com/watch?v={video}"
                        ))
                        .unwrap()
                        .0,
                        published_at: at("2026-03-01T20:00:00+09:00"),
                        scheduled_start_at: Some(at("2026-03-01T21:00:00+09:00")),
                        content: Content::Media {
                            media_type: MediaType::YoutubeLive,
                            title: format!("○○の雑談配信 {video}"),
                        },
                        media: MediaPresence::Present,
                    },
                    at("2026-03-01T20:05:00+09:00"),
                )
                .await
                .unwrap();

            let seq = store
                .append(
                    id,
                    Seq::FIRST,
                    &Event::AcquisitionStarted,
                    at("2026-03-01T20:10:00+09:00"),
                )
                .await
                .unwrap();
            store
                .append(
                    id,
                    seq,
                    &Event::Acquired {
                        transcript: TranscriptNeed::NotNeeded,
                        hold: Hold::Until(deadline),
                    },
                    at("2026-03-02T00:30:00+09:00"),
                )
                .await
                .unwrap();
            ids.push(id);
        }

        let before: Vec<_> = {
            let mut kept = Vec::new();
            for id in &ids {
                kept.push(store.item(*id).await.unwrap().unwrap());
            }
            kept
        };
        assert_eq!(before[0].item.state, State::Holding { until: deadline });

        // 手で壊す — 状態を書き換え、1件は行ごと消す。
        sqlx::query("UPDATE item SET state = 'error', hold_until = NULL")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM item WHERE id = ?")
            .bind(i64::from(ids[1]))
            .execute(&store.pool)
            .await
            .unwrap();

        assert_eq!(store.rebuild().await.unwrap(), 2);

        for (id, expected) in ids.iter().zip(&before) {
            assert_eq!(&store.item(*id).await.unwrap().unwrap(), expected);
        }
    }

    /// 引き渡し済みの項目と、繋がりを持つ投稿も、作り直しで元に戻ること。
    ///
    /// 状態と対でしか意味を持たない値（`release_reference`）と、subtype ごとの値
    /// （`title` / `body` / 繋がりの URL）が、どれもログから戻ることの確認（#1）。
    #[tokio::test]
    async fn released_items_and_linked_posts_survive_a_rebuild() {
        let (store, source) = crate::testing::seeded().await;

        // 引き渡し済みの配信。題名に引用符と日本語が入る。
        let live = store
            .discover(
                &Discovered {
                    source_id: source,
                    url: url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                        .unwrap()
                        .0,
                    published_at: at("2026-03-01T20:00:00+09:00"),
                    scheduled_start_at: Some(at("2026-03-01T21:00:00+09:00")),
                    content: Content::Media {
                        media_type: MediaType::YoutubeLive,
                        title: "「89%」が言う \"widespread\" — ○○の雑談".to_owned(),
                    },
                    media: MediaPresence::Present,
                },
                at("2026-03-01T20:05:00+09:00"),
            )
            .await
            .unwrap();

        let mut seq = Seq::FIRST;
        for (event, moment) in [
            (Event::AcquisitionStarted, "2026-03-01T20:10:00+09:00"),
            (
                Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: Hold::None,
                },
                "2026-03-02T00:30:00+09:00",
            ),
            (
                Event::Released {
                    reference: Some(escrow_domain::state::ReleaseReference::new(
                        "Attachments/2026-03-01 ○○.mp4",
                    )),
                },
                "2026-03-03T21:00:00+09:00",
            ),
        ] {
            seq = store.append(live, seq, &event, at(moment)).await.unwrap();
        }

        // 繋がりを持つ投稿。実体が無いので kept から始まる。
        let post = store
            .discover(
                &crate::testing::a_post(source),
                at("2026-03-01T12:01:00+09:00"),
            )
            .await
            .unwrap();

        let before = [
            store.item(live).await.unwrap().unwrap(),
            store.item(post).await.unwrap().unwrap(),
        ];

        assert_eq!(store.rebuild().await.unwrap(), 2);

        for (id, expected) in [live, post].into_iter().zip(&before) {
            assert_eq!(&store.item(id).await.unwrap().unwrap(), expected);
        }
    }

    /// ログが空なら、作り直しても空。
    #[tokio::test]
    async fn rebuilding_an_empty_log_leaves_an_empty_read_model() {
        let store = EventStore::open_in_memory().await.unwrap();
        assert_eq!(store.rebuild().await.unwrap(), 0);
        assert!(
            store
                .items_in_state(escrow_domain::state::StateName::Waiting)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

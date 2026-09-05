//! 監視対象（#1）。

use std::num::NonZeroU32;

use escrow_domain::source::{Monitoring, PersonId, Source, SourceId};
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::{self, NormalizedUrl};

use crate::{Ledger, LedgerError, RowError, positive, source_timestamp};

/// 配信元を登録するときに渡すもの。`id` はまだ無い。
#[derive(Debug, Clone)]
pub struct NewSource {
    pub person_id: PersonId,
    pub url: NormalizedUrl,
    pub enabled: bool,
    pub created_at: Timestamp,
    /// これから取得するものを何日預かるかの既定値。進行中の預かりには遡らない（#1）。
    pub hold_days: Option<NonZeroU32>,
    pub priority: NonZeroU32,
    pub monitoring: Monitoring,
}

impl Ledger {
    pub async fn add_source(&self, source: &NewSource) -> Result<SourceId, LedgerError> {
        let person_id = i64::from(source.person_id);
        let url = source.url.as_str();
        let enabled = i64::from(source.enabled);
        let created_at = source.created_at.to_text();
        let hold_days = source.hold_days.map(|d| i64::from(d.get()));
        let priority = i64::from(source.priority.get());
        let (monitor_from, monitor_until) = source.monitoring.columns();
        let monitor_from = monitor_from.map(Timestamp::to_text);
        let monitor_until = monitor_until.map(Timestamp::to_text);

        let id = sqlx::query!(
            "INSERT INTO source (person_id, url, enabled, created_at, priority, hold_days, \
             monitor_from, monitor_until) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             RETURNING id AS \"id!\"",
            person_id,
            url,
            enabled,
            created_at,
            priority,
            hold_days,
            monitor_from,
            monitor_until,
        )
        .fetch_one(&self.pool)
        .await?
        .id;

        Ok(SourceId::new(id))
    }

    pub async fn source(&self, id: SourceId) -> Result<Option<Source>, LedgerError> {
        let key = i64::from(id);
        let row = sqlx::query!(
            r#"SELECT id AS "id!", person_id, url, enabled AS "enabled: bool", created_at,
                      priority, hold_days, monitor_from, monitor_until
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
            enabled: row.enabled,
            created_at: source_timestamp(row.id, "created_at", Some(&row.created_at))?
                .expect("created_at は NOT NULL"),
            hold_days: positive(row.hold_days, "source", row.id, "hold_days")?,
            priority: positive(Some(row.priority), "source", row.id, "priority")?.ok_or(
                RowError::OutOfRange {
                    table: "source",
                    id: row.id,
                    column: "priority",
                    value: row.priority,
                },
            )?,
            monitoring: Monitoring::new(
                source_timestamp(row.id, "monitor_from", row.monitor_from.as_deref())?,
                source_timestamp(row.id, "monitor_until", row.monitor_until.as_deref())?,
            )
            .map_err(|source| RowError::BadMonitoring { id: row.id, source })?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use escrow_domain::source::MonitoringError;
    use escrow_domain::state::Hold;

    use crate::testing::{at, seeded};
    use crate::{Ledger, LedgerError, NewSource, RowError};
    use escrow_domain::source::{Monitoring, PersonId};
    use escrow_domain::url;

    #[tokio::test]
    async fn round_trips_a_source() {
        let (ledger, source_id) = seeded().await;
        let source = ledger.source(source_id).await.unwrap().unwrap();

        assert_eq!(source.hold_days, NonZeroU32::new(7));
        assert_eq!(source.priority.get(), 1);
        assert_eq!(source.monitoring, Monitoring::Continuous);
        assert!(source.enabled);

        // 既定値から、取得の時点で1つの期限になる（#1）。
        assert_eq!(
            source.hold_from(at("2026-03-02T00:30:00+09:00")).unwrap(),
            Hold::Until(at("2026-03-09T00:30:00+09:00")),
        );
    }

    /// 同じ配信元を二度登録できない。できてしまうと、二個目は item.url の
    /// UNIQUE に阻まれて検知が毎回空振りする。
    #[tokio::test]
    async fn the_same_source_cannot_be_registered_twice() {
        let (ledger, source_id) = seeded().await;
        let source = ledger.source(source_id).await.unwrap().unwrap();
        let person = ledger.add_person("△△").await.unwrap();

        let again = ledger
            .add_source(&NewSource {
                person_id: person,
                url: source.url.clone(),
                enabled: true,
                created_at: at("2026-02-01T00:00:00+09:00"),
                hold_days: None,
                priority: NonZeroU32::MIN,
                monitoring: Monitoring::Continuous,
            })
            .await;

        assert!(again.is_err(), "別の持ち主でも同じ配信元は二度入らない");
    }

    /// #1 の「持ち主のいない `Source` は作れない」。
    #[tokio::test]
    async fn a_source_without_an_owner_is_refused() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let orphan = ledger
            .add_source(&NewSource {
                person_id: PersonId::new(999),
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled: true,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days: None,
                priority: NonZeroU32::MIN,
                monitoring: Monitoring::Continuous,
            })
            .await;

        assert!(orphan.is_err(), "外部キーが効くこと");
    }

    /// 監視の期間が2列を往復すること（#1）。
    #[tokio::test]
    async fn round_trips_a_monitoring_period() {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let person = ledger.add_person("○○").await.unwrap();
        let from = at("2026-09-01T00:00:00+09:00");
        let until = at("2026-09-08T00:00:00+09:00");

        let id = ledger
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source("https://x.com/i/user/12").unwrap(),
                enabled: true,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days: None,
                priority: NonZeroU32::new(3).unwrap(),
                monitoring: Monitoring::new(Some(from), Some(until)).unwrap(),
            })
            .await
            .unwrap();

        let source = ledger.source(id).await.unwrap().unwrap();
        assert_eq!(source.priority.get(), 3);
        assert_eq!(source.monitoring, Monitoring::Period { from, until });
        assert_eq!(
            source.hold_from(at("2026-09-02T00:00:00+09:00")).unwrap(),
            Hold::None,
            "hold_days が空なら捨てない"
        );
    }

    /// 片方だけ埋まった行は、意味が決まっていないので読み出しが撥ねる。
    ///
    /// DB は2列に分かれているが、写した先の [`Monitoring`] は「両方空」か
    /// 「両方埋まっている」しか持てない。その差をここで受け止める。
    #[tokio::test]
    async fn refuses_half_of_a_monitoring_period() {
        for corruption in [
            "UPDATE source SET monitor_from = '2026-09-01T00:00:00+09:00'",
            "UPDATE source SET monitor_until = '2026-09-01T00:00:00+09:00'",
        ] {
            let (ledger, id) = seeded().await;
            sqlx::query(corruption).execute(&ledger.pool).await.unwrap();

            match ledger.source(id).await {
                Err(LedgerError::Row(RowError::BadMonitoring {
                    source: MonitoringError::HalfOpen,
                    ..
                })) => {}
                other => panic!("{corruption} が通ってしまった: {other:?}"),
            }
        }
    }

    /// 終わりが先に来る期間も撥ねる。
    #[tokio::test]
    async fn refuses_a_monitoring_period_that_ends_before_it_starts() {
        let (ledger, id) = seeded().await;
        sqlx::query(
            "UPDATE source SET monitor_from = '2026-09-08T00:00:00+09:00', \
             monitor_until = '2026-09-01T00:00:00+09:00'",
        )
        .execute(&ledger.pool)
        .await
        .unwrap();

        assert!(matches!(
            ledger.source(id).await,
            Err(LedgerError::Row(RowError::BadMonitoring {
                source: MonitoringError::NotOrdered,
                ..
            }))
        ));
    }
}

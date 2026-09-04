//! 取り込まない種別（#1）。

use std::str::FromStr;

use escrow_domain::content::ContentType;
use escrow_domain::source::{Exclude, ExcludeId, SourceId};

use crate::{Ledger, LedgerError, RowError};

impl Ledger {
    pub async fn add_exclude(
        &self,
        source_id: Option<SourceId>,
        content_type: ContentType,
        enabled: bool,
    ) -> Result<ExcludeId, LedgerError> {
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
    pub async fn excludes(&self) -> Result<Vec<Exclude>, LedgerError> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", source_id, content_type, enabled AS "enabled: bool"
               FROM exclude ORDER BY id"#
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
                    enabled: row.enabled,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use escrow_domain::content::ContentType;

    use crate::testing::seeded;

    #[tokio::test]
    async fn round_trips_excludes() {
        let (ledger, source) = seeded().await;
        ledger
            .add_exclude(Some(source), ContentType::XSpace, true)
            .await
            .unwrap();
        ledger
            .add_exclude(None, ContentType::YoutubeShorts, true)
            .await
            .unwrap();

        let excludes = ledger.excludes().await.unwrap();
        assert_eq!(excludes.len(), 2);
        assert_eq!(excludes[0].source_id, Some(source));
        assert_eq!(excludes[1].source_id, None, "共通条件は source_id が空");
    }
}

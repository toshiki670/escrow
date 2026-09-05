//! 配信元の持ち主（#1）。

use escrow_domain::source::{Person, PersonId};

use crate::{Ledger, LedgerError};

impl Ledger {
    pub async fn add_person(&self, name: &str) -> Result<PersonId, LedgerError> {
        let id = sqlx::query!(
            "INSERT INTO person (name) VALUES (?) RETURNING id AS \"id!\"",
            name
        )
        .fetch_one(&self.pool)
        .await?
        .id;

        Ok(PersonId::new(id))
    }

    pub async fn person(&self, id: PersonId) -> Result<Option<Person>, LedgerError> {
        let key = i64::from(id);
        let row = sqlx::query!(r#"SELECT id AS "id!", name FROM person WHERE id = ?"#, key)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|row| Person {
            id: PersonId::new(row.id),
            name: row.name,
        }))
    }
}

#[cfg(test)]
mod tests {
    use escrow_domain::state::StateName;

    use crate::testing::{a_holding_item, seeded};

    /// #1 の削除の連鎖。`PERSON` を消すと、その `SOURCE`・`ITEM`・`ITEM_EVENT` が消える。
    ///
    /// 事象は投影を参照していないので、連鎖は `source_id` の側から届く。届かないと、
    /// 投影だけが消えてログが残り、`rebuild` で消したはずのものが甦る。
    #[tokio::test]
    async fn deleting_a_person_takes_its_sources_items_and_events() {
        let (ledger, source) = seeded().await;
        let id = a_holding_item(&ledger, source).await;

        sqlx::query("DELETE FROM person")
            .execute(&ledger.pool)
            .await
            .unwrap();

        assert!(ledger.source(source).await.unwrap().is_none());
        assert!(
            ledger
                .items_in_state(StateName::Holding)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(ledger.log(id).await.unwrap().is_none(), "ログも消える");

        // 作り直しても甦らない。
        assert_eq!(ledger.rebuild().await.unwrap(), 0);
    }
}

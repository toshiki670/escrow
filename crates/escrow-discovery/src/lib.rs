//! 配信元から、まだ見ていない項目を見つける（#15 のスライス）。
//!
//! 見つけたものを台帳へ起票するところまで。取得はしない — 起票すれば状態が
//! `waiting` になり、次に誰が拾うかは状態が決める（#15 の Blackboard）。
//!
//! 巡回の間隔・優先度による重み付け・予算との兼ね合いは Phase 5 と 6（#7・#13）。
//! ここに在るのは「1つの配信元を1回見る」だけで、**いつ見るかは持たない**。

use escrow_domain::item::{Discovered, ItemId};
use escrow_domain::source::{Exclude, Source};
use escrow_domain::timestamp::Timestamp;
use escrow_ledger::{Ledger, LedgerError};
use escrow_scheduler::{AdapterError, Discover};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
}

pub struct Discovery<'a, D> {
    ledger: &'a Ledger,
    discover: &'a D,
}

impl<'a, D: Discover> Discovery<'a, D> {
    pub const fn new(ledger: &'a Ledger, discover: &'a D) -> Self {
        Self { ledger, discover }
    }

    /// 1つの配信元を1回見て、新しく見つけたものを起票する。
    ///
    /// 見ない場合が2つある。どちらも #1 が決めたことで、**行を作らずに済ませる**。
    ///
    /// - 配信元が無効
    /// - 監視の期間の外。X はこの期間の中だけ見る（#5）ので、期間を持つ配信元では
    ///   外に居る間は外へも出ない
    ///
    /// 除外に当たった種別も行を作らない。除外されていることは [`Exclude`] が持つ（#1）。
    /// 既に台帳に在るものは飛ばす — 判定は `Item.url` の一意キー。
    pub async fn sweep(
        &self,
        source: &Source,
        excludes: &[Exclude],
        now: Timestamp,
    ) -> Result<Vec<ItemId>, DiscoveryError> {
        if !source.enabled || !source.monitoring.covers(now) {
            return Ok(Vec::new());
        }

        // #1 のとおり監視対象は `Source.created_at` 以降。
        let found = self.discover.discover(source, source.created_at).await?;

        let mut started = Vec::new();
        for found in found {
            let content_type = found.content_type();
            if excludes.iter().any(|e| e.covers(source.id, content_type)) {
                continue;
            }
            if self.ledger.item_by_url(&found.url).await?.is_some() {
                continue;
            }

            let id = self
                .ledger
                .discover(
                    &Discovered {
                        source_id: source.id,
                        url: found.url,
                        published_at: found.published_at,
                        scheduled_start_at: found.scheduled_start_at,
                        content: found.content,
                        media: found.media,
                    },
                    now,
                )
                .await?;
            started.push(id);
        }

        Ok(started)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_domain::content::{Content, ContentType, MediaType};
    use escrow_domain::source::{ExcludeId, Monitoring, SourceId};
    use escrow_domain::state::MediaPresence;
    use escrow_domain::url;
    use escrow_ledger::NewSource;
    use escrow_scheduler::Found;
    use std::num::NonZeroU32;

    /// 配信元が返す代わりのもの。スケジューラが見せている口だけを満たす。
    struct FakeDiscover(Vec<Found>);

    impl Discover for FakeDiscover {
        async fn discover(
            &self,
            _source: &Source,
            _since: Timestamp,
        ) -> Result<Vec<Found>, AdapterError> {
            Ok(self.0.clone())
        }
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    fn found(video_id: &str, media_type: MediaType) -> Found {
        Found {
            url: url::normalize_item(&format!("https://www.youtube.com/watch?v={video_id}"))
                .unwrap()
                .0,
            published_at: at("2026-03-01T20:00:00+09:00"),
            scheduled_start_at: None,
            content: Content::Media {
                media_type,
                title: format!("○○の配信 {video_id}"),
            },
            media: MediaPresence::Present,
        }
    }

    async fn seeded(monitoring: Monitoring, enabled: bool) -> (Ledger, Source) {
        let ledger = Ledger::open_in_memory().await.unwrap();
        let person = ledger.add_person("○○").await.unwrap();
        let id = ledger
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days: None,
                priority: NonZeroU32::MIN,
                monitoring,
            })
            .await
            .unwrap();
        let source = ledger.source(id).await.unwrap().unwrap();
        (ledger, source)
    }

    fn exclude(source_id: Option<SourceId>, content_type: ContentType) -> Exclude {
        Exclude {
            id: ExcludeId::new(1),
            source_id,
            content_type,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn found_items_are_written_to_the_ledger() {
        let (ledger, source) = seeded(Monitoring::Continuous, true).await;
        let discover = FakeDiscover(vec![
            found("dQw4w9WgXcQ", MediaType::YoutubeVideo),
            found("bLKBe3uMMRI", MediaType::YoutubeLive),
        ]);

        let started = Discovery::new(&ledger, &discover)
            .sweep(&source, &[], at("2026-03-01T20:05:00+09:00"))
            .await
            .unwrap();

        assert_eq!(started.len(), 2);
        for id in started {
            let item = ledger.item(id).await.unwrap().unwrap().item;
            assert_eq!(item.source_id, source.id);
            assert_eq!(item.state.name(), escrow_domain::state::StateName::Waiting);
        }
    }

    /// 除外に当たったものは**行を作らない**（#1）。
    #[tokio::test]
    async fn excluded_kinds_never_become_rows() {
        let (ledger, source) = seeded(Monitoring::Continuous, true).await;
        let discover = FakeDiscover(vec![
            found("dQw4w9WgXcQ", MediaType::YoutubeVideo),
            found("bLKBe3uMMRI", MediaType::YoutubeShorts),
        ]);

        let started = Discovery::new(&ledger, &discover)
            .sweep(
                &source,
                &[exclude(None, ContentType::YoutubeShorts)],
                at("2026-03-01T20:05:00+09:00"),
            )
            .await
            .unwrap();

        assert_eq!(started.len(), 1);
        let item = ledger.item(started[0]).await.unwrap().unwrap().item;
        assert_eq!(item.content_type(), ContentType::YoutubeVideo);
    }

    /// 二度目の巡回で同じものを見つけても、行は増えない（#1 の一意キー）。
    #[tokio::test]
    async fn sweeping_twice_does_not_duplicate() {
        let (ledger, source) = seeded(Monitoring::Continuous, true).await;
        let discover = FakeDiscover(vec![found("dQw4w9WgXcQ", MediaType::YoutubeVideo)]);
        let discovery = Discovery::new(&ledger, &discover);

        let first = discovery
            .sweep(&source, &[], at("2026-03-01T20:05:00+09:00"))
            .await
            .unwrap();
        let second = discovery
            .sweep(&source, &[], at("2026-03-01T21:05:00+09:00"))
            .await
            .unwrap();

        assert_eq!(first.len(), 1);
        assert!(second.is_empty(), "二度目は起票しない");
    }

    /// 監視の期間の外では、外へも出ない（#1・#5）。
    #[tokio::test]
    async fn nothing_happens_outside_the_monitoring_period() {
        let period = Monitoring::new(
            Some(at("2026-09-01T00:00:00+09:00")),
            Some(at("2026-09-08T00:00:00+09:00")),
        )
        .unwrap();
        let (ledger, source) = seeded(period, true).await;
        let discover = FakeDiscover(vec![found("dQw4w9WgXcQ", MediaType::YoutubeVideo)]);

        let outside = Discovery::new(&ledger, &discover)
            .sweep(&source, &[], at("2026-03-01T20:05:00+09:00"))
            .await
            .unwrap();
        assert!(outside.is_empty());

        let inside = Discovery::new(&ledger, &discover)
            .sweep(&source, &[], at("2026-09-03T20:05:00+09:00"))
            .await
            .unwrap();
        assert_eq!(inside.len(), 1);
    }

    #[tokio::test]
    async fn a_disabled_source_is_not_visited() {
        let (ledger, source) = seeded(Monitoring::Continuous, false).await;
        let discover = FakeDiscover(vec![found("dQw4w9WgXcQ", MediaType::YoutubeVideo)]);

        let started = Discovery::new(&ledger, &discover)
            .sweep(&source, &[], at("2026-03-01T20:05:00+09:00"))
            .await
            .unwrap();
        assert!(started.is_empty());
    }
}

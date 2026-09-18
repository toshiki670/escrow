//! テストの共通部品。
//!
//! リードモデルを壊す・行を消すといった確認には、接続が要る。**公開 API に
//! 接続が出ていない**ことの裏返しで、だからこの手のテストは crate の中に置く。

use std::num::NonZeroU32;

use escrow_domain::content::{Content, MediaType};
use escrow_domain::item::{Discovered, ItemId};
use escrow_domain::source::{Monitoring, SourceId};
use escrow_domain::state::{Event, Hold, MediaPresence, TranscriptNeed};
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::{self, NormalizedUrl};

use crate::{EventStore, NewSource, Seq};

pub(crate) fn at(text: &str) -> Timestamp {
    Timestamp::parse(text).expect(text)
}

pub(crate) fn item_url(raw: &str) -> NormalizedUrl {
    url::normalize_item(raw).expect(raw).0
}

/// 人と配信元を1つずつ持つ DB。項目には FK が要るので、これが最小の準備。
pub(crate) async fn seeded() -> (EventStore, SourceId) {
    let store = EventStore::open_in_memory().await.unwrap();
    let source = seed_into(&store).await;
    (store, source)
}

pub(crate) async fn seed_into(store: &EventStore) -> SourceId {
    let person = store.add_person("○○").await.unwrap();
    store
        .add_source(&NewSource {
            person_id: person,
            url: url::normalize_source("https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ")
                .unwrap(),
            enabled: true,
            created_at: at("2026-01-01T00:00:00+09:00"),
            hold_days: NonZeroU32::new(7),
            priority: NonZeroU32::MIN,
            monitoring: Monitoring::Continuous,
        })
        .await
        .unwrap()
}

pub(crate) fn a_live(source_id: SourceId) -> Discovered {
    Discovered {
        source_id,
        url: item_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
        published_at: at("2026-03-01T20:00:00+09:00"),
        scheduled_start_at: None,
        content: Content::Media {
            media_type: MediaType::YoutubeLive,
            title: "○○の雑談配信".to_owned(),
        },
        media: MediaPresence::Present,
    }
}

pub(crate) fn a_post(source_id: SourceId) -> Discovered {
    Discovered {
        source_id,
        url: item_url("https://x.com/jack/status/20"),
        published_at: at("2026-03-01T12:00:00+09:00"),
        scheduled_start_at: None,
        content: Content::Post {
            body: "明日の配信は21時から。".to_owned(),
            in_reply_to: Some(item_url("https://x.com/someone/status/19")),
            quoted: Some(item_url("https://youtu.be/dQw4w9WgXcQ")),
        },
        media: MediaPresence::Absent,
    }
}

/// 配信を1本、`holding` まで運ぶ。期限は 2026-03-09T00:30:00+09:00。
pub(crate) async fn a_holding_item(store: &EventStore, source: SourceId) -> ItemId {
    let id = store
        .discover(&a_live(source), at("2026-03-01T20:05:00+09:00"))
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
                hold: Hold::Until(at("2026-03-09T00:30:00+09:00")),
            },
            at("2026-03-02T00:30:00+09:00"),
        )
        .await
        .unwrap();

    id
}

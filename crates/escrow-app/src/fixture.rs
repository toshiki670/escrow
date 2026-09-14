//! 入口のテストが台帳へ置く形（#82）。feature `fixture` で公開する。
//!
//! 入口は `escrow-ledger` を名前で知らない（`tests/dependency_direction.rs`）ので、
//! 台帳を仕込むのもここの仕事。入口のテストは [`App`] を受け取り、描いた結果だけを見る。

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use escrow_config::{Config, Paths, Resolver};
use escrow_domain::content::{Content, MediaType};
use escrow_domain::item::Discovered;
use escrow_domain::source::{Monitoring, PersonId, SourceId};
use escrow_domain::state::{Event, Hold, MediaPresence, TranscriptNeed};
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url;
use escrow_ledger::{Ledger, NewSource, Seq};

use crate::App;

fn at(text: &str) -> Timestamp {
    Timestamp::parse(text).expect(text)
}

async fn a_source_for(ledger: &Ledger, person: PersonId, raw: &str) -> SourceId {
    ledger
        .add_source(&NewSource {
            person_id: person,
            url: url::normalize_source(raw).expect(raw),
            enabled: true,
            created_at: at("2026-01-01T00:00:00+09:00"),
            hold_days: NonZeroU32::new(7),
            priority: NonZeroU32::MIN,
            monitoring: Monitoring::Continuous,
        })
        .await
        .unwrap()
}

impl App {
    /// CLI の `person add` → `source add` → `item add` → `fetch` が作る形を、台帳へ直接置く。
    ///
    /// ○○ は配信1本（`holding`）と投稿1件（`kept`）、□□ は項目を持たない。
    ///
    /// **新しいほうを後から見つける。** 投影は id の順で返るので、こうしないと
    /// 「並べ替えを忘れた」が「たまたま合っている」に化ける。
    ///
    /// 台帳はメモリの上に在り、実体の置き場所だけを `media_dir` で受ける。設定は既定で、
    /// 外部ツールは1つも見つからない — **支えるのは読む側だけ**で、`add_item` と `fetch`
    /// はここでは通らない。
    pub async fn seeded(media_dir: &Path) -> Self {
        let ledger = Ledger::open_in_memory().await.unwrap();

        let owner = ledger.add_person("○○").await.unwrap();
        let youtube = a_source_for(
            &ledger,
            owner,
            "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
        )
        .await;
        let x = a_source_for(&ledger, owner, "https://x.com/i/user/12").await;

        // 本文だけの投稿は、取るものが無いのでそのまま kept から始まる（#1）。
        ledger
            .discover(
                &Discovered {
                    source_id: x,
                    url: url::normalize_item("https://x.com/jack/status/20")
                        .unwrap()
                        .0,
                    published_at: at("2026-03-01T12:00:00+09:00"),
                    scheduled_start_at: None,
                    content: Content::Post {
                        body: "明日の配信は21時から。".to_owned(),
                        in_reply_to: None,
                        quoted: None,
                    },
                    media: MediaPresence::Absent,
                },
                at("2026-03-01T12:01:00+09:00"),
            )
            .await
            .unwrap();

        let live = ledger
            .discover(
                &Discovered {
                    source_id: youtube,
                    url: url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                        .unwrap()
                        .0,
                    published_at: at("2026-03-01T20:00:00+09:00"),
                    scheduled_start_at: None,
                    content: Content::Media {
                        media_type: MediaType::YoutubeLive,
                        title: "○○の雑談配信".to_owned(),
                    },
                    media: MediaPresence::Present,
                },
                at("2026-03-01T20:05:00+09:00"),
            )
            .await
            .unwrap();
        let seq = ledger
            .append(
                live,
                Seq::FIRST,
                &Event::AcquisitionStarted,
                at("2026-03-01T20:10:00+09:00"),
            )
            .await
            .unwrap();
        ledger
            .append(
                live,
                seq,
                &Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: Hold::Until(at("2026-03-09T00:30:00+09:00")),
                },
                at("2026-03-02T00:30:00+09:00"),
            )
            .await
            .unwrap();

        ledger.add_person("□□").await.unwrap();

        Self {
            config: Config::default(),
            paths: Paths {
                config_file: PathBuf::new(),
                db: PathBuf::new(),
                media_dir: media_dir.to_owned(),
                transcribe_model: PathBuf::new(),
            },
            ledger,
            resolver: Resolver::new(None, &[]),
        }
    }
}

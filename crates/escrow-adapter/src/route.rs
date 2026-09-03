//! #5 の対応表。**プラットフォーム × 操作 → ツール**。
//!
//! | | 検知 | 取得 |
//! |---|---|---|
//! | YouTube ショート / 動画 / 配信 | RSS ＋ yt-dlp の追加取得 | yt-dlp |
//! | X 投稿 | gallery-dl | gallery-dl |
//! | X Space | gallery-dl | yt-dlp |
//! | X ライブ配信 | gallery-dl | yt-dlp |
//!
//! **この対応をコードの1か所に置く。** 散らばっていると「X の取得を別ツールへ」が
//! 何箇所の変更になるか分からない。ここに集めてあれば、変わるのは表の1行になる。
//!
//! 呼ぶ側（パイプライン・CLI・エンジン）はどのツールが動いたかを知らない。

use std::path::Path;

use crate::gallerydl::GalleryDl;
use crate::rss::Rss;
use crate::ytdlp::YtDlp;
use escrow_core::adapter::{Acquire, AdapterError, Discover, Found};
use escrow_core::asset::Asset;
use escrow_core::content::{Content, ContentType, Platform};
use escrow_core::source::Source;
use escrow_core::state::MediaPresence;
use escrow_core::timestamp::Timestamp;
use escrow_core::url::{self, NormalizedUrl, TypeHint};

/// 使えるツールを揃えたもの。
///
/// **中のツールを外へ見せない。** 見せると呼ぶ側が対応表を迂回して
/// `adapters.ytdlp` を直に叩けてしまい、「この対応をコードの1か所に置く」が
/// 約束にしかならない。外から呼べるのは下の3つだけ。
pub struct Adapters {
    rss: Rss,
    ytdlp: YtDlp,
    gallerydl: GalleryDl,
}

impl Adapters {
    pub const fn new(rss: Rss, ytdlp: YtDlp, gallerydl: GalleryDl) -> Self {
        Self {
            rss,
            ytdlp,
            gallerydl,
        }
    }

    /// 対応表の「取得」列。
    ///
    /// X の Space とライブ配信が gallery-dl でないのは、`twitter:spaces` /
    /// `twitter:broadcast` を持つのが yt-dlp だけだから（#5）。
    pub const fn acquirer(&self, content_type: ContentType) -> Acquirer<'_> {
        match content_type {
            ContentType::YoutubeShorts | ContentType::YoutubeVideo | ContentType::YoutubeLive => {
                Acquirer::YtDlp(&self.ytdlp)
            }
            ContentType::XPost => Acquirer::GalleryDl(&self.gallerydl),
            ContentType::XSpace | ContentType::XBroadcast => Acquirer::YtDlp(&self.ytdlp),
        }
    }

    /// 対応表の「検知」列。
    ///
    /// YouTube は RSS。認証が要らず、1チャンネル1回で済み、叩いてよい頻度が
    /// 公表されている（#5）。X は yt-dlp ではタイムラインを列挙できないので
    /// gallery-dl。
    pub fn discoverer(&self, source: &Source) -> Result<Discoverer<'_>, AdapterError> {
        match url::platform_of_source(&source.url) {
            Some(Platform::Youtube) => Ok(Discoverer::Youtube {
                rss: &self.rss,
                ytdlp: &self.ytdlp,
            }),
            Some(Platform::X) => Ok(Discoverer::GalleryDl(&self.gallerydl)),
            None => Err(AdapterError::Parse {
                program: "escrow".to_owned(),
                detail: format!("どのプラットフォームの配信元か決められない: {}", source.url),
            }),
        }
    }

    /// 1件のメタデータを取る。
    ///
    /// 分かれ目は subtype。`Media` は yt-dlp、`Post` は gallery-dl。本文と
    /// 繋がりの URL を返せるのが gallery-dl だけだから（#5）。
    pub async fn describe(
        &self,
        url: &NormalizedUrl,
        content_type: ContentType,
    ) -> Result<Found, AdapterError> {
        match content_type.media_type() {
            Some(media_type) => self.ytdlp.describe(url, media_type).await,
            None => self.gallerydl.describe(url).await,
        }
    }
}

/// 取得を担うもの。呼ぶ側はどれが動いたかを知らない。
pub enum Acquirer<'a> {
    YtDlp(&'a YtDlp),
    GalleryDl(&'a GalleryDl),
}

impl Acquire for Acquirer<'_> {
    async fn acquire(
        &self,
        url: &NormalizedUrl,
        content_type: ContentType,
        into: &Path,
    ) -> Result<Vec<Asset>, AdapterError> {
        match self {
            Self::YtDlp(tool) => tool.acquire(url, content_type, into).await,
            Self::GalleryDl(tool) => tool.acquire(url, content_type, into).await,
        }
    }
}

/// 検知を担うもの。
pub enum Discoverer<'a> {
    /// **RSS で見つけ、足りないぶんだけ yt-dlp で埋める**（#5）。
    ///
    /// フィードは1チャンネル1回で15件を返すが、`/watch?v=` の項目が動画か配信かを
    /// 語らず、開始時刻も持たない。そこだけを1件ごとの追加取得で埋める。
    Youtube {
        rss: &'a Rss,
        ytdlp: &'a YtDlp,
    },
    GalleryDl(&'a GalleryDl),
}

impl Discover for Discoverer<'_> {
    async fn discover(
        &self,
        source: &Source,
        since: Timestamp,
    ) -> Result<Vec<Found>, AdapterError> {
        match self {
            Self::Youtube { rss, ytdlp } => discover_youtube(rss, ytdlp, source, since).await,
            Self::GalleryDl(tool) => tool.discover(source, since).await,
        }
    }
}

/// フィードを読み、`since` 以降のものだけ追加取得へ回す。
///
/// **追加取得の回数を決めるのは `since`。** フィードは常に15件返すので、絞らないと
/// 巡回のたびに15回叩くことになる。台帳との突き合わせは呼ぶ側の仕事なので（#1 の
/// `Item.url` の一意キー）、ここでは日時だけで切る。
async fn discover_youtube(
    rss: &Rss,
    ytdlp: &YtDlp,
    source: &Source,
    since: Timestamp,
) -> Result<Vec<Found>, AdapterError> {
    let mut found = Vec::new();

    for sighting in rss.sightings(&source.url).await? {
        if sighting.published_at < since {
            continue;
        }

        // ショートはフィードだけで決まる。追加取得は要らない。
        let (media_type, scheduled_start_at) = match sighting.hint {
            TypeHint::Known(content_type) => (
                content_type
                    .media_type()
                    .ok_or_else(|| AdapterError::Parse {
                        program: "escrow".to_owned(),
                        detail: format!("YouTube に本文だけの種別は無い: {content_type}"),
                    })?,
                None,
            ),
            TypeHint::YoutubeUnknown => {
                let schedule = ytdlp.schedule(&sighting.url).await?;
                (schedule.media_type, schedule.scheduled_start_at)
            }
        };

        found.push(Found {
            url: sighting.url,
            // 枠を作った時刻。予約枠の開始予定時刻とは別物なので、追加取得の値で
            // 上書きしない（#5）。
            published_at: sighting.published_at,
            scheduled_start_at,
            content: Content::Media {
                media_type,
                title: sighting.title,
            },
            // YouTube のものは、どれも落とす実体を持つ。
            media: MediaPresence::Present,
        });
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_core::config::Browser;
    use escrow_core::source::{Monitoring, PersonId, SourceId};
    use std::num::NonZeroU32;

    fn adapters() -> Adapters {
        Adapters::new(
            Rss::new(),
            YtDlp::new("/bin/yt-dlp", Browser::Firefox),
            GalleryDl::new("/bin/gallery-dl", Browser::Firefox),
        )
    }

    fn source(url: &str) -> Source {
        Source {
            id: SourceId::new(1),
            person_id: PersonId::new(1),
            url: url::normalize_source(url).expect(url),
            enabled: true,
            created_at: Timestamp::parse("2026-01-01T00:00:00+09:00").unwrap(),
            hold_days: None,
            priority: NonZeroU32::new(1).unwrap(),
            monitoring: Monitoring::Continuous,
        }
    }

    fn acquirer_name(acquirer: &Acquirer<'_>) -> &'static str {
        match acquirer {
            Acquirer::YtDlp(_) => "yt-dlp",
            Acquirer::GalleryDl(_) => "gallery-dl",
        }
    }

    fn discoverer_name(discoverer: &Discoverer<'_>) -> &'static str {
        match discoverer {
            Discoverer::Youtube { .. } => "rss",
            Discoverer::GalleryDl(_) => "gallery-dl",
        }
    }

    /// #5 の対応表の「取得」列をそのまま写したもの。表が動いたらここが落ちる。
    #[test]
    fn acquisition_follows_the_table() {
        let adapters = adapters();

        for (content_type, expected) in [
            (ContentType::YoutubeShorts, "yt-dlp"),
            (ContentType::YoutubeVideo, "yt-dlp"),
            (ContentType::YoutubeLive, "yt-dlp"),
            (ContentType::XPost, "gallery-dl"),
            // Space とライブ配信は X だが、extractor を持つのは yt-dlp だけ。
            (ContentType::XSpace, "yt-dlp"),
            (ContentType::XBroadcast, "yt-dlp"),
        ] {
            assert_eq!(
                acquirer_name(&adapters.acquirer(content_type)),
                expected,
                "{content_type}"
            );
        }
    }

    /// 「検知」列。YouTube は RSS、X は gallery-dl（#5）。
    #[test]
    fn discovery_follows_the_table() {
        let adapters = adapters();

        let youtube = source("https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ");
        assert_eq!(
            discoverer_name(&adapters.discoverer(&youtube).unwrap()),
            "rss"
        );

        let x = source("https://x.com/i/user/12");
        assert_eq!(
            discoverer_name(&adapters.discoverer(&x).unwrap()),
            "gallery-dl"
        );
    }

    /// 全種別に取得の担当がいること。種別を足したらここで気づく。
    #[test]
    fn every_type_has_someone_to_fetch_it() {
        let adapters = adapters();
        for content_type in ContentType::ALL {
            let _ = adapters.acquirer(content_type);
        }
    }

    #[test]
    fn a_type_belongs_to_exactly_one_platform() {
        for content_type in ContentType::ALL {
            let expected = if content_type.as_str().starts_with("youtube") {
                Platform::Youtube
            } else {
                Platform::X
            };
            assert_eq!(content_type.platform(), expected, "{content_type}");
        }
    }
}

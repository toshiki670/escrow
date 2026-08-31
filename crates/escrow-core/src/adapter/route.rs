//! #5 の対応表。**プラットフォーム × 操作 → ツール**。
//!
//! | | 検知 | 取得 |
//! |---|---|---|
//! | YouTube ショート / 動画 / 配信 | yt-dlp | yt-dlp |
//! | X 投稿 | gallery-dl | gallery-dl |
//! | X Space | gallery-dl | yt-dlp |
//! | X ライブ配信 | gallery-dl | yt-dlp |
//!
//! **この対応をコードの1か所に置く。** 散らばっていると「X の取得を別ツールへ」が
//! 何箇所の変更になるか分からない。ここに集めてあれば、変わるのは表の1行になる。
//!
//! 呼ぶ側（パイプライン・CLI・エンジン）はどのツールが動いたかを知らない。

use std::path::Path;

use super::gallerydl::GalleryDl;
use super::ytdlp::YtDlp;
use super::{Acquire, AdapterError, Discover, Found};
use crate::asset::Asset;
use crate::content::{ContentType, Platform};
use crate::source::Source;
use crate::timestamp::Timestamp;
use crate::url::{self, NormalizedUrl};

/// 使えるツールを揃えたもの。
pub struct Adapters {
    pub ytdlp: YtDlp,
    pub gallerydl: GalleryDl,
}

impl Adapters {
    pub const fn new(ytdlp: YtDlp, gallerydl: GalleryDl) -> Self {
        Self { ytdlp, gallerydl }
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
    /// X は yt-dlp ではタイムラインを列挙できないので gallery-dl（#5）。
    pub fn discoverer(&self, source: &Source) -> Result<Discoverer<'_>, AdapterError> {
        match url::platform_of_source(&source.url) {
            Some(Platform::Youtube) => Ok(Discoverer::YtDlp(&self.ytdlp)),
            Some(Platform::X) => Ok(Discoverer::GalleryDl(&self.gallerydl)),
            None => Err(AdapterError::Parse {
                program: "escrow".to_owned(),
                detail: format!("どのプラットフォームの配信元か決められない: {}", source.url),
            }),
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
    YtDlp(&'a YtDlp),
    GalleryDl(&'a GalleryDl),
}

impl Discover for Discoverer<'_> {
    async fn discover(
        &self,
        source: &Source,
        since: Timestamp,
    ) -> Result<Vec<Found>, AdapterError> {
        match self {
            Self::YtDlp(tool) => tool.discover(source, since).await,
            Self::GalleryDl(tool) => tool.discover(source, since).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Browser;
    use crate::source::{PersonId, SourceId};
    use std::num::NonZeroU32;

    fn adapters() -> Adapters {
        Adapters::new(
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
            discover_interval_minutes: NonZeroU32::new(15).unwrap(),
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
            Discoverer::YtDlp(_) => "yt-dlp",
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

    /// 「検知」列。X は yt-dlp ではタイムラインを列挙できない（#5）。
    #[test]
    fn discovery_follows_the_table() {
        let adapters = adapters();

        let youtube = source("https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ");
        assert_eq!(
            discoverer_name(&adapters.discoverer(&youtube).unwrap()),
            "yt-dlp"
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

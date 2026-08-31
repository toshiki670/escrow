//! #5 の対応表。**プラットフォーム × 操作 → ツール**。
//!
//! | | 検知 | 取得 | 生存確認 |
//! |---|---|---|---|
//! | YouTube ショート / 動画 / 配信 | yt-dlp | yt-dlp | yt-dlp |
//! | X 投稿 | gallery-dl | gallery-dl | **未定** |
//! | X Space | gallery-dl | yt-dlp | **未定** |
//! | X ライブ配信 | gallery-dl | yt-dlp | **未定** |
//!
//! 生存確認の列は #5 が yt-dlp しか書いていない。X のぶんは [`Prober::Undecided`]
//! で、表から抜けているのではなく**決まっていない**ことを型で持つ。
//!
//! **この対応をコードの1か所に置く。** 散らばっていると「X の取得を別ツールへ」が
//! 何箇所の変更になるか分からない。ここに集めてあれば、変わるのは表の1行になる。
//!
//! 呼ぶ側（パイプライン・CLI・エンジン）はどのツールが動いたかを知らない。

use std::path::Path;

use super::gallerydl::GalleryDl;
use super::whisper::Whisper;
use super::ytdlp::YtDlp;
use super::{Acquire, AdapterError, Discover, Found, Ports, Probe, Transcribe};
use crate::asset::Asset;
use crate::content::{ContentType, Platform};
use crate::liveness::Presence;
use crate::source::Source;
use crate::timestamp::Timestamp;
use crate::url::{self, NormalizedUrl};

/// 使えるツールを揃えたもの。
pub struct Adapters {
    pub ytdlp: YtDlp,
    pub gallerydl: GalleryDl,
    pub whisper: Whisper,
}

impl Adapters {
    pub const fn new(ytdlp: YtDlp, gallerydl: GalleryDl, whisper: Whisper) -> Self {
        Self {
            ytdlp,
            gallerydl,
            whisper,
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

    /// 対応表の「生存確認」列。
    ///
    /// X のぶんを #5 が決めていないので [`Prober::Undecided`]。返さないのではなく
    /// 「確かめられない」を返すので、#5 の非対称性の規則がそのまま効く —
    /// 在ることを確かめられない項目は `holding` に留まり、捨てられない。
    pub const fn prober(&self, content_type: ContentType) -> Prober<'_> {
        match content_type {
            ContentType::YoutubeShorts | ContentType::YoutubeVideo | ContentType::YoutubeLive => {
                Prober::YtDlp(&self.ytdlp)
            }
            ContentType::XPost | ContentType::XSpace | ContentType::XBroadcast => Prober::Undecided,
        }
    }
}

/// [`Ports`] の実装。エンジンが外の世界へ触れるのはここだけで、
/// #5 の対応表もここを通る。
impl Ports for Adapters {
    fn discoverer(&self, source: &Source) -> Result<impl Discover, AdapterError> {
        Self::discoverer(self, source)
    }

    fn acquirer(&self, content_type: ContentType) -> impl Acquire {
        Self::acquirer(self, content_type)
    }

    fn prober(&self, content_type: ContentType) -> impl Probe {
        Self::prober(self, content_type)
    }

    fn transcriber(&self) -> impl Transcribe {
        &self.whisper
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

/// 生存確認を担うもの。
pub enum Prober<'a> {
    YtDlp(&'a YtDlp),
    /// 手段が決まっていない。#5 の「生存確認」は yt-dlp のことしか書いていない。
    ///
    /// 空を返さず、この変種が [`Presence::Unknown`] を返す。#5 の
    /// 「それ以外すべては判定保留」に当たるので、呼ぶ側に分岐が要らず、
    /// 手段が無い配信元を黙って捨てにいく経路も生まれない。
    Undecided,
}

impl Probe for Prober<'_> {
    async fn probe(&self, url: &NormalizedUrl) -> Result<Presence, AdapterError> {
        match self {
            Self::YtDlp(tool) => tool.probe(url).await,
            Self::Undecided => Ok(Presence::Unknown),
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
            Whisper::new(
                "/bin/whisper-cli",
                "/bin/ffmpeg",
                "/models/ggml.bin",
                crate::config::Language::Code("ja".to_owned()),
            ),
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

    /// 「生存確認」列。X は #5 が決めていないので、手段が無いことを持つ。
    #[test]
    fn liveness_follows_the_table() {
        let adapters = adapters();

        for content_type in ContentType::ALL {
            let decided = matches!(adapters.prober(content_type), Prober::YtDlp(_));
            assert_eq!(
                decided,
                content_type.platform() == Platform::Youtube,
                "{content_type}"
            );
        }
    }

    /// 手段が無い配信元は、消えたとは答えない（#5 の非対称性）。
    #[tokio::test]
    async fn an_undecided_prober_never_says_gone() {
        let url = url::normalize_item("https://x.com/jack/status/20")
            .unwrap()
            .0;
        assert_eq!(
            Prober::Undecided.probe(&url).await.unwrap(),
            Presence::Unknown
        );
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

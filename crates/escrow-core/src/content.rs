//! 項目の種別と中身。#1 の「種別」表と subtype に対応する。

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::url::NormalizedUrl;

/// #1 の種別。subtype の判別子を兼ねる。
///
/// 値はプラットフォーム名を含み、プラットフォームをまたいで重複しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContentType {
    YoutubeShorts,
    YoutubeVideo,
    YoutubeLive,
    XPost,
    XSpace,
    XBroadcast,
}

impl ContentType {
    /// #1 の値表。DB と #4 の JSON に出る文字列。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::YoutubeShorts => "youtube_shorts",
            Self::YoutubeVideo => "youtube_video",
            Self::YoutubeLive => "youtube_live",
            Self::XPost => "x_post",
            Self::XSpace => "x_space",
            Self::XBroadcast => "x_broadcast",
        }
    }

    /// 種別が決まれば中身の形も決まる。#1 の subtype 列。
    pub const fn shape(self) -> ContentShape {
        match self {
            Self::YoutubeShorts | Self::YoutubeVideo | Self::YoutubeLive => ContentShape::Media,
            Self::XPost => ContentShape::Post,
            Self::XSpace | Self::XBroadcast => ContentShape::Media,
        }
    }

    /// 巡回や網羅テストのために全種別を並べる。
    pub const ALL: [Self; 6] = [
        Self::YoutubeShorts,
        Self::YoutubeVideo,
        Self::YoutubeLive,
        Self::XPost,
        Self::XSpace,
        Self::XBroadcast,
    ];
}

/// 中身の形。`Post` も画像や動画を持つので、境目は**本文の枠があるかどうか**（#1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentShape {
    Media,
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("escrow が知らない種別: {0}")]
pub struct UnknownContentType(pub String);

impl FromStr for ContentType {
    type Err = UnknownContentType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| UnknownContentType(s.to_owned()))
    }
}

impl fmt::Display for ContentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 項目の中身。
///
/// 「`Media` に `body` は無い」を守るのは Rust の enum であって DB ではない（#1）。
/// DB は平らなカラムを持つだけで、`CHECK` 制約も置かない。保証はここ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// 見出しを持つ項目。YouTube の全種別、X Space、X ライブ配信。
    Media { title: String },
    /// 本文と繋がりを持つ項目。X 投稿。
    Post {
        /// 動画だけの投稿では空文字列になる。「無い」のではなく「空」。
        body: String,
        in_reply_to: Option<NormalizedUrl>,
        quoted: Option<NormalizedUrl>,
    },
}

impl Content {
    pub const fn shape(&self) -> ContentShape {
        match self {
            Self::Media { .. } => ContentShape::Media,
            Self::Post { .. } => ContentShape::Post,
        }
    }

    /// 種別と中身が噛み合っているか。境界で行を組み立てるときに使う。
    pub fn matches(&self, content_type: ContentType) -> bool {
        self.shape() == content_type.shape()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1 の種別表をそのまま写したもの。表が動いたらここが落ちる。
    const TABLE: [(ContentType, &str, ContentShape); 6] = [
        (
            ContentType::YoutubeShorts,
            "youtube_shorts",
            ContentShape::Media,
        ),
        (
            ContentType::YoutubeVideo,
            "youtube_video",
            ContentShape::Media,
        ),
        (
            ContentType::YoutubeLive,
            "youtube_live",
            ContentShape::Media,
        ),
        (ContentType::XPost, "x_post", ContentShape::Post),
        (ContentType::XSpace, "x_space", ContentShape::Media),
        (ContentType::XBroadcast, "x_broadcast", ContentShape::Media),
    ];

    #[test]
    fn matches_the_type_table() {
        assert_eq!(TABLE.len(), ContentType::ALL.len(), "ALL に漏れがある");

        for (content_type, value, shape) in TABLE {
            assert_eq!(content_type.as_str(), value);
            assert_eq!(content_type.shape(), shape);
            assert_eq!(value.parse::<ContentType>().unwrap(), content_type);
        }
    }

    #[test]
    fn rejects_values_it_does_not_know() {
        assert!("youtube_livestream".parse::<ContentType>().is_err());
        assert!("".parse::<ContentType>().is_err());
    }

    #[test]
    fn shape_must_agree_with_the_discriminator() {
        let media = Content::Media {
            title: "○○の雑談配信".to_owned(),
        };
        let post = Content::Post {
            body: "明日の配信は21時から。".to_owned(),
            in_reply_to: None,
            quoted: None,
        };

        assert!(media.matches(ContentType::YoutubeLive));
        assert!(!media.matches(ContentType::XPost));
        assert!(post.matches(ContentType::XPost));
        assert!(!post.matches(ContentType::XSpace));
    }
}

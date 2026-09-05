//! 項目の種別と中身。#1 の「種別」表と subtype に対応する。

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::url::NormalizedUrl;

/// #1 の種別。subtype の判別子を兼ねる。
///
/// 値はプラットフォーム名を含み、プラットフォームをまたいで重複しない。
/// 中身を持たない場面（`Exclude` の対象、#6 の絞り込み、DB の列）でも使うので、
/// [`Content`] とは別に立っている。
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

    /// どのプラットフォームのものか。値がプラットフォーム名を含むので一意に決まる。
    pub const fn platform(self) -> Platform {
        match self {
            Self::YoutubeShorts | Self::YoutubeVideo | Self::YoutubeLive => Platform::Youtube,
            Self::XPost | Self::XSpace | Self::XBroadcast => Platform::X,
        }
    }

    /// subtype が `Media` ならその種別。`Post` 側（`x_post`）は `None`。
    pub const fn media_type(self) -> Option<MediaType> {
        match self {
            Self::YoutubeShorts => Some(MediaType::YoutubeShorts),
            Self::YoutubeVideo => Some(MediaType::YoutubeVideo),
            Self::YoutubeLive => Some(MediaType::YoutubeLive),
            Self::XSpace => Some(MediaType::XSpace),
            Self::XBroadcast => Some(MediaType::XBroadcast),
            Self::XPost => None,
        }
    }

    pub const ALL: [Self; 6] = [
        Self::YoutubeShorts,
        Self::YoutubeVideo,
        Self::YoutubeLive,
        Self::XPost,
        Self::XSpace,
        Self::XBroadcast,
    ];
}

/// escrow が扱うプラットフォーム。
///
/// #1 のとおり DB には持たない（`url` から判別できるため）。#5 の対応表が
/// 「どのツールを使うか」をこれで引く。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Platform {
    Youtube,
    X,
}

impl Platform {
    pub const ALL: [Self; 2] = [Self::Youtube, Self::X];
}

/// subtype が `Media` になる種別。#1 の表で `Media` 側の5つ。
///
/// [`Content::Media`] がこれを持つことで、`content_type` と中身の食い違いが
/// **表現できなくなる**。両方を並べて持って突き合わせる、という形にしない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MediaType {
    YoutubeShorts,
    YoutubeVideo,
    YoutubeLive,
    XSpace,
    XBroadcast,
}

impl MediaType {
    pub const fn content_type(self) -> ContentType {
        match self {
            Self::YoutubeShorts => ContentType::YoutubeShorts,
            Self::YoutubeVideo => ContentType::YoutubeVideo,
            Self::YoutubeLive => ContentType::YoutubeLive,
            Self::XSpace => ContentType::XSpace,
            Self::XBroadcast => ContentType::XBroadcast,
        }
    }
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
///
/// 種別は中身から導ける（[`Content::content_type`]）ので、並べて持たない。
/// #1 の「計算・導出できるものは持たない」。
///
/// `Post` も画像や動画を持つので、境目は「メディアを持つほう」ではなく
/// **本文の枠があるかどうか**（#1）。動画だけの X 投稿は `body` が空の `Post`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// 見出しを持つ項目。
    Media {
        media_type: MediaType,
        title: String,
    },
    /// 本文と繋がりを持つ項目。いまは `x_post` だけがこの形。
    Post {
        /// 動画だけの投稿では空文字列になる。「無い」のではなく「空」。
        body: String,
        in_reply_to: Option<NormalizedUrl>,
        quoted: Option<NormalizedUrl>,
    },
}

impl Content {
    pub const fn content_type(&self) -> ContentType {
        match self {
            Self::Media { media_type, .. } => media_type.content_type(),
            Self::Post { .. } => ContentType::XPost,
        }
    }

    /// #6 の一覧に出す見出し。`Media` は `title`、`Post` は `body`。
    ///
    /// 何文字で切るか改行をどう畳むかは表示する側が決めるので、ここでは丸ごと返す（#4）。
    pub fn headline(&self) -> &str {
        match self {
            Self::Media { title, .. } => title,
            Self::Post { body, .. } => body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1 の種別表をそのまま写したもの。表が動いたらここが落ちる。
    const TABLE: [(ContentType, &str, Option<MediaType>); 6] = [
        (
            ContentType::YoutubeShorts,
            "youtube_shorts",
            Some(MediaType::YoutubeShorts),
        ),
        (
            ContentType::YoutubeVideo,
            "youtube_video",
            Some(MediaType::YoutubeVideo),
        ),
        (
            ContentType::YoutubeLive,
            "youtube_live",
            Some(MediaType::YoutubeLive),
        ),
        (ContentType::XPost, "x_post", None),
        (ContentType::XSpace, "x_space", Some(MediaType::XSpace)),
        (
            ContentType::XBroadcast,
            "x_broadcast",
            Some(MediaType::XBroadcast),
        ),
    ];

    #[test]
    fn matches_the_type_table() {
        assert_eq!(TABLE.len(), ContentType::ALL.len(), "ALL に漏れがある");

        for (content_type, value, media_type) in TABLE {
            assert_eq!(content_type.as_str(), value);
            assert_eq!(content_type.media_type(), media_type);
            assert_eq!(value.parse::<ContentType>().unwrap(), content_type);
        }
    }

    #[test]
    fn rejects_values_it_does_not_know() {
        assert!("youtube_livestream".parse::<ContentType>().is_err());
        assert!("".parse::<ContentType>().is_err());
    }

    /// 種別と中身は往復する。`Media` 側の5つはどれも同じ値へ戻る。
    #[test]
    fn content_carries_its_own_type() {
        for (content_type, _, media_type) in TABLE {
            let content = match media_type {
                Some(media_type) => Content::Media {
                    media_type,
                    title: "○○の雑談配信".to_owned(),
                },
                None => Content::Post {
                    body: "明日の配信は21時から。".to_owned(),
                    in_reply_to: None,
                    quoted: None,
                },
            };
            assert_eq!(content.content_type(), content_type);
        }
    }

    #[test]
    fn headline_comes_from_whichever_field_the_shape_has() {
        let media = Content::Media {
            media_type: MediaType::YoutubeLive,
            title: "○○の雑談配信".to_owned(),
        };
        let post = Content::Post {
            body: "明日の配信は21時から。".to_owned(),
            in_reply_to: None,
            quoted: None,
        };

        assert_eq!(media.headline(), "○○の雑談配信");
        assert_eq!(post.headline(), "明日の配信は21時から。");
    }
}

//! 見つけた1件と、その現在の状態。台帳を兼ね、手放した後も行は残る（#1）。

use derive_more::{Constructor, Display, Into};

use crate::content::{Content, ContentType};
use crate::source::SourceId;
use crate::state::{MediaPresence, State};
use crate::timestamp::Timestamp;
use crate::url::NormalizedUrl;

/// `Item` の外部ハンドル。
///
/// #4 の `escrow release <id>` が受け取る値で、実体の置き場所もここから導出される（#1）。
/// `url` が自然キーで、こちらは外へ見せる同一性。
///
/// **`From<i64>` は出さない。** `i64` から作る道は [`ItemId::new`] だけにしておくと、
/// `impl Into<ItemId>` を取る場所へ裸の主キーが推論で滑り込むことがない。逆向きの
/// `i64::from` は DB へ渡すのに要るので出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Constructor, Display, Into)]
pub struct ItemId(i64);

/// 台帳の1行。
///
/// `content_type` を別に持たないのは、[`Content`] から導けるため（#1 の
/// 「計算・導出できるものは持たない」）。並べて持つと、噛み合わない組を
/// 作れてしまう。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub source_id: SourceId,
    /// 正規化した URL。項目の一意キー。
    pub url: NormalizedUrl,
    pub published_at: Timestamp,
    /// 配信の開始予定時刻。予約枠でなければ空。
    ///
    /// 予約枠を見つけた時点で分かり、始まってしまえば二度と取れない。巻き戻し
    /// 禁止の配信は開始に間に合わせないと頭を失うので、この時刻に取得を予約する
    /// （#1・#5）。
    pub scheduled_start_at: Option<Timestamp>,
    /// いまの状態。預かりの期限は `Holding` が伴っている（#1）。
    pub state: State,
    /// この状態になった日時。**状態が変わらなかった事象では動かない** — 生存確認や
    /// 1回の失敗を書いても、`holding` になった日時はそのまま。
    pub state_since: Timestamp,
    pub content: Content,
}

impl Item {
    pub fn content_type(&self) -> ContentType {
        self.content.content_type()
    }
}

/// 見つけた時点で分かっていること。**ログの先頭にちょうど1つ**置かれる（#1）。
///
/// 状態を動かすのではなく作るので、[`crate::state::Event`] には入らない。これが
/// 無いと `url` や `title` が投影の側にしか存在せず、投影を捨てて作り直せない。
///
/// [`Item`] との差は `id` と `state` と `state_since` の3つで、どれも誕生の時点では
/// まだ決まっていない — `id` は DB が採番し、残る2つはログを畳んで出る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub source_id: SourceId,
    /// 正規化した URL。項目の一意キー。
    pub url: NormalizedUrl,
    pub published_at: Timestamp,
    /// 配信の開始予定時刻。予約枠でなければ空。
    pub scheduled_start_at: Option<Timestamp>,
    pub content: Content,
    /// 取得する実体があるか。`Content` からは導けない — 画像だけの X 投稿と
    /// テキストだけの X 投稿は、どちらも `Post` で本文を持つ（#1）。
    pub media: MediaPresence,
}

impl Discovered {
    /// 誕生の直後の状態。#1 の `[*]` から出る2本。
    pub const fn initial_state(&self) -> State {
        State::initial(self.media)
    }

    pub fn content_type(&self) -> ContentType {
        self.content.content_type()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::MediaType;

    fn item(content: Content) -> Item {
        Item {
            id: ItemId::new(42),
            source_id: SourceId::new(1),
            url: crate::url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                .unwrap()
                .0,
            published_at: Timestamp::parse("2026-03-01T20:00:00+09:00").unwrap(),
            scheduled_start_at: None,
            state: State::Kept,
            state_since: Timestamp::parse("2026-03-01T22:30:00+09:00").unwrap(),
            content,
        }
    }

    fn discovered(content: Content, media: MediaPresence) -> Discovered {
        Discovered {
            source_id: SourceId::new(1),
            url: crate::url::normalize_item("https://x.com/i/status/20")
                .unwrap()
                .0,
            published_at: Timestamp::parse("2026-03-01T20:00:00+09:00").unwrap(),
            scheduled_start_at: None,
            content,
            media,
        }
    }

    /// 実体の有無は `Content` から導けない。どちらも本文の枠を持つ `Post`（#1）。
    #[test]
    fn whether_there_is_media_is_not_written_in_the_content() {
        let post = Content::Post {
            body: "明日の配信は21時から。".to_owned(),
            in_reply_to: None,
            quoted: None,
        };

        let with_image = discovered(post.clone(), MediaPresence::Present);
        let text_only = discovered(post, MediaPresence::Absent);

        assert_eq!(with_image.content, text_only.content);
        assert_eq!(with_image.initial_state(), State::Waiting);
        assert_eq!(text_only.initial_state(), State::Kept);
    }

    #[test]
    fn the_type_comes_from_the_content() {
        let media = item(Content::Media {
            media_type: MediaType::YoutubeLive,
            title: "○○の雑談配信".to_owned(),
        });
        assert_eq!(media.content_type(), ContentType::YoutubeLive);

        let post = item(Content::Post {
            body: "明日の配信は21時から。".to_owned(),
            in_reply_to: None,
            quoted: None,
        });
        assert_eq!(post.content_type(), ContentType::XPost);
    }
}

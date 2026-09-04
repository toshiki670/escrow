//! 見つけた1件と、その現在の状態。台帳を兼ね、手放した後も行は残る（#1）。

use crate::content::{Content, ContentType};
use crate::id::id_type;
use crate::source::SourceId;
use crate::state::State;
use crate::timestamp::Timestamp;
use crate::url::NormalizedUrl;

id_type! {
    /// `Item` の外部ハンドル。
    ///
    /// #4 の `escrow release <id>` が受け取る値で、実体の置き場所もここから導出される（#1）。
    /// `url` が自然キーで、こちらは外へ見せる同一性。
    ItemId
}

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
    pub state: State,
    /// この状態になった日時。預かりの期限は `Source.hold_days` と合わせて計算する。
    pub state_since: Timestamp,
    pub content: Content,
}

impl Item {
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

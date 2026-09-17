//! 一覧に出す1件（#6）。
//!
//! 状態と種別は [`Handed`] から取る。#4 が外部へ返す値と同じ綴りを画面へ出すためで、
//! 組み立てる場所を1つにしておかないと、画面に出る値と外部が受け取る値がずれる。
//!
//! 日付だけはリードモデルの [`Timestamp`] から取る。[`Handed`] の `published_at` は text
//! なので、そこから読み直すと**失敗しようのない経路に失敗の余地ができる**。
//!
//! 見出しを何文字で切るかは、列幅の都合なので入口が決める（#6「切り方を決めるのは
//! 表示する側」）。ここが返すのは1行目の全部で、`title` か `body` の1行目かの区別を
//! 付けて返す — 入口が切るかどうかを、その区別で決める（#82）。

use std::cmp::Reverse;

use escrow_domain::timestamp::Timestamp;
use escrow_handover::Handed;

/// 一覧の1行 — 日付・見出し・状態・種別（#6）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    published_at: Timestamp,
    headline: Headline,
    state: String,
    content_type: String,
}

/// 見出し。`Media` は `title`、`Post` は `body` の1行目（#6）。
///
/// どちらから来たかを型で持つのは、入口が切るかどうかをそれで決めるため（#82）。
/// `title` は全文のまま、`body` の1行目だけを列幅で切る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Headline {
    /// `Media` の `title`。全文が見出し。
    Title(String),
    /// `Post` の `body` の1行目。
    Opening(String),
}

impl Listed {
    pub(crate) fn new(published_at: Timestamp, handed: &Handed) -> Self {
        Self {
            published_at,
            headline: headline(handed),
            state: handed.state.clone(),
            content_type: handed.content_type.clone(),
        }
    }

    /// 一覧に出す日付。配信元が言った時差のまま読む。
    pub fn published_on(&self) -> String {
        self.published_at.inner().date_naive().to_string()
    }

    pub const fn headline(&self) -> &Headline {
        &self.headline
    }

    /// #1 の状態表の値。
    pub fn state(&self) -> &str {
        &self.state
    }

    /// #1 の種別表の値。
    pub fn content_type(&self) -> &str {
        &self.content_type
    }
}

/// 新しいものから順に並べる（#6 のモック）。
///
/// 並べ替えを SQL へ渡さないのは、`published_at` が時差を保つ text だからで、
/// 字面の順は時刻の順にならない（#1）。[`Timestamp`] は瞬間で比べる。
pub(crate) fn newest_first(listed: &mut [Listed]) {
    listed.sort_by_key(|listed| Reverse(listed.published_at));
}

/// #4 はどちらか片方だけを埋めると決めているが、[`Handed`] の型は両方空も表せる。
/// **そこは締めていない**ので、両方空なら空の1行目になる。
fn headline(handed: &Handed) -> Headline {
    match (handed.title.as_deref(), handed.body.as_deref()) {
        (Some(title), _) => Headline::Title(title.to_owned()),
        (None, Some(body)) => Headline::Opening(body.lines().next().unwrap_or_default().to_owned()),
        (None, None) => Headline::Opening(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    /// #4 の `list` が返す1件。埋める側を入れ替えて `Media` と `Post` を作り分ける。
    fn handed(title: Option<&str>, body: Option<&str>, published_at: &str) -> Handed {
        Handed {
            id: 42,
            title: title.map(str::to_owned),
            body: body.map(str::to_owned),
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_owned(),
            content_type: "youtube_live".to_owned(),
            state: "kept".to_owned(),
            published_at: at(published_at).to_text(),
            media_paths: Vec::new(),
            transcript_paths: Vec::new(),
        }
    }

    fn listed(title: Option<&str>, body: Option<&str>, published_at: &str) -> Listed {
        Listed::new(at(published_at), &handed(title, body, published_at))
    }

    /// 見出しの決め方（#6）。`Media` は `title` をそのまま、`Post` は本文の先頭。
    #[test]
    fn the_headline_comes_from_whichever_field_the_shape_fills() {
        let media = listed(Some("○○の雑談配信"), None, "2026-03-01T20:00:00+09:00");
        assert_eq!(
            media.headline(),
            &Headline::Title("○○の雑談配信".to_owned())
        );

        let post = listed(
            None,
            Some("明日の配信は21時から。"),
            "2026-03-01T12:00:00+09:00",
        );
        assert_eq!(
            post.headline(),
            &Headline::Opening("明日の配信は21時から。".to_owned())
        );
    }

    /// 本文は1行目だけを出す。改行から先は一覧の高さを崩す。
    #[test]
    fn a_body_contributes_only_its_first_line() {
        let post = listed(
            None,
            Some("明日の配信は21時から。\n遅れたらごめん"),
            "2026-03-01T12:00:00+09:00",
        );
        assert_eq!(
            post.headline(),
            &Headline::Opening("明日の配信は21時から。".to_owned())
        );
    }

    /// 長い本文も1行目の全部を返す。切るのは入口（#6）。
    #[test]
    fn a_long_body_keeps_its_whole_first_line() {
        let body = "あ".repeat(100);
        let post = listed(None, Some(&body), "2026-03-01T12:00:00+09:00");

        assert_eq!(post.headline(), &Headline::Opening(body));
    }

    /// 日付は日付だけ。時差は畳まず、配信元が言った日のまま出す（#1）。
    #[test]
    fn the_date_keeps_the_offset_it_was_published_with() {
        let jst = listed(Some("○○の雑談配信"), None, "2026-03-01T08:00:00+09:00");
        assert_eq!(jst.published_on(), "2026-03-01");

        // 同じ瞬間でも、UTC で言われたなら前日になる。
        let utc = listed(Some("○○の雑談配信"), None, "2026-02-28T23:00:00+00:00");
        assert_eq!(utc.published_on(), "2026-02-28");
    }

    /// 状態と種別は #4 の綴りをそのまま通す（#1 の表の値）。
    #[test]
    fn the_state_and_the_content_type_pass_through_untouched() {
        let one = listed(Some("○○の雑談配信"), None, "2026-03-01T20:00:00+09:00");

        assert_eq!(one.state(), "kept");
        assert_eq!(one.content_type(), "youtube_live");
    }

    /// 一覧は新しいものが上（#6 のモック）。時差の違う行も瞬間で並ぶ。
    #[test]
    fn the_newest_item_comes_first() {
        let mut listing = vec![
            listed(Some("古い"), None, "2026-02-28T23:00:00+00:00"),
            // 字面はこれが一番大きい。瞬間で見ると 11:00 UTC で、真ん中に来る。
            listed(Some("真ん中"), None, "2026-03-01T20:00:00+09:00"),
            listed(Some("新しい"), None, "2026-03-01T12:00:00+00:00"),
        ];
        newest_first(&mut listing);

        let headlines: Vec<&Headline> = listing.iter().map(Listed::headline).collect();
        assert_eq!(
            headlines,
            [
                &Headline::Title("新しい".to_owned()),
                &Headline::Title("真ん中".to_owned()),
                &Headline::Title("古い".to_owned()),
            ]
        );
    }
}

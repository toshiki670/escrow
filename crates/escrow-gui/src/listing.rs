//! 一覧の見出しを列幅に合わせて切る（#6）。
//!
//! 見出しそのもの（`title`、無ければ `body` の1行目）は `escrow-app` が決める。何文字で
//! 切るかは列幅の都合で、入口ごとに変わる — 切り方を決めるのは表示する側（#6）。
//! 切るのは `body` の1行目だけで、`title` は全文のまま（#82）。

use escrow_app::Headline;

/// 本文の1行目に載せる文字数。これを越えたぶんは落として `…` を付ける。
const HEADLINE_CHARS: usize = 60;

/// 一覧の列に載せる文字。`title` は全文、`body` の1行目は [`HEADLINE_CHARS`] 文字まで。
pub fn shown(headline: &Headline) -> String {
    match headline {
        Headline::Title(title) => title.clone(),
        Headline::Opening(opening) => cut(opening),
    }
}

fn cut(opening: &str) -> String {
    let mut shown: String = opening.chars().take(HEADLINE_CHARS).collect();

    if opening.chars().count() > HEADLINE_CHARS {
        shown.push('…');
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 長い `title` は全文のまま（#82）。列幅で切るのは本文の1行目だけ。
    #[test]
    fn a_long_title_is_shown_in_full() {
        let title = "あ".repeat(HEADLINE_CHARS + 10);

        assert_eq!(shown(&Headline::Title(title.clone())), title);
    }

    /// 長い本文の1行目は文字の境目で切る。バイトで切ると日本語が壊れる。
    #[test]
    fn a_long_opening_is_cut_at_a_character_boundary() {
        let opening = "あ".repeat(HEADLINE_CHARS + 10);
        let shown = shown(&Headline::Opening(opening));

        assert_eq!(shown.chars().count(), HEADLINE_CHARS + 1);
        assert!(shown.ends_with('…'));
    }

    /// ちょうど収まるぶんには `…` を付けない。
    #[test]
    fn an_opening_that_fits_keeps_its_ending() {
        let opening = "あ".repeat(HEADLINE_CHARS);

        assert_eq!(shown(&Headline::Opening(opening.clone())), opening);
    }
}

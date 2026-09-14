//! 一覧の見出しを列幅に合わせて切る（#6）。
//!
//! 見出しそのもの（`title`、無ければ `body` の1行目）は `escrow-app` が決める。何文字で
//! 切るかは列幅の都合で、入口ごとに変わる — 切り方を決めるのは表示する側（#6）。

/// 見出しに載せる文字数。これを越えたぶんは落として `…` を付ける。
const HEADLINE_CHARS: usize = 60;

/// 見出しを [`HEADLINE_CHARS`] 文字まで。
pub fn cut(headline: &str) -> String {
    let mut shown: String = headline.chars().take(HEADLINE_CHARS).collect();

    if headline.chars().count() > HEADLINE_CHARS {
        shown.push('…');
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 長い見出しは文字の境目で切る。バイトで切ると日本語が壊れる。
    #[test]
    fn a_long_headline_is_cut_at_a_character_boundary() {
        let headline = "あ".repeat(HEADLINE_CHARS + 10);
        let shown = cut(&headline);

        assert_eq!(shown.chars().count(), HEADLINE_CHARS + 1);
        assert!(shown.ends_with('…'));
    }

    /// ちょうど収まるぶんには `…` を付けない。
    #[test]
    fn a_headline_that_fits_keeps_its_ending() {
        let headline = "あ".repeat(HEADLINE_CHARS);

        assert_eq!(cut(&headline), headline);
    }
}

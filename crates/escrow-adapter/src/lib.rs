//! 外部ツールの呼び出し。
//!
//! `escrow-core` が語彙（[`escrow_core::adapter`] の trait と型）を決め、
//! ここがそれを実際のプロセス起動へ落とす。#5 が決めた対応表の実装にあたる層で、
//! #5 自身が「ここで決めるのは実装の詳細で、データモデル（#1）より可変性が高い」と
//! 言っているとおり、**ここは変わる前提**で組む。X の仕様変更、ツールのフラグ変更、
//! ツールそのものの入れ替えに耐えること。
//!
//! **この crate を依存に持つのは `escrow-scheduler` だけ**（#3）。core も UI も
//! 外部ツールを名前で知らないので、外部アクセスの迂回はコンパイルエラーになる。
//!
//! # 分け方
//!
//! 3つの層を混ぜない。それぞれ別の理由で壊れるため。
//!
//! | 層 | 形 | 壊れる理由 |
//! |---|---|---|
//! | 引数の組み立て | 純関数 → [`Invocation`] | ツールのフラグが変わった |
//! | 出力の読み取り | 純関数 `&str -> Result<_, AdapterError>` | ツールの出力形式が変わった |
//! | 実行 | [`run`] 1か所 | OS 側の事情 |
//!
//! 1つの関数に混ぜると、落ちたときにどれが原因か分からない。分けてあれば、
//! 引数のテストはプロセスを起動せず argv を突き合わせるだけで済み、出力のテストは
//! 実物を固めた fixture で offline に回せる。

pub mod gallerydl;
pub mod invocation;
pub mod route;
pub mod rss;
pub mod whisper;
pub mod ytdlp;

pub use invocation::{Completed, Invocation, run};
pub use route::{Acquirer, Adapters, Discoverer};

#[cfg(test)]
mod tests {
    use escrow_core::config::Browser;

    /// 設定に並ぶブラウザは、**すべてのアダプタが受けられる**こと。
    ///
    /// #2 が「認証の取得元はプラットフォームごとに分けない」と決めたので、1つの値が
    /// 全アダプタへ渡る。どれか1つでも受けないものが混ざると、そのプラットフォームだけ
    /// 落ちる。アダプタを足すときは、ここへ1行足して同じことを確かめる。
    #[test]
    fn every_configurable_browser_works_with_every_adapter() {
        let adapters: [(&str, &[Browser]); 2] = [
            ("yt-dlp", super::ytdlp::SUPPORTED_BROWSERS),
            ("gallery-dl", super::gallerydl::SUPPORTED_BROWSERS),
        ];

        for browser in Browser::ALL {
            for (name, supported) in adapters {
                assert!(
                    supported.contains(&browser),
                    "{name} は {browser} を受けないので、共通設定に置けない"
                );
            }
        }
    }
}

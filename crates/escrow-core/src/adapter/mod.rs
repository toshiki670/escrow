//! 外部ツールとの境界。
//!
//! #5 が決めた対応表を実装に落とす層。#5 自身が「ここで決めるのは実装の詳細で、
//! データモデル（#1）より可変性が高い」と言っているとおり、**ここは変わる前提**で
//! 組む。X の仕様変更、ツールのフラグ変更、ツールそのものの入れ替えに耐えること。
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
//!
//! # 口はドメインの語彙で切る
//!
//! [`Discover`] / [`Acquire`] / [`Transcribe`] / [`Probe`] のどれにも、ツール固有の
//! 語を出さない。「タイムラインを列挙する」ではなく「配信元から、まだ見ていない
//! 項目を見つける」。ツールを入れ替えても、呼ぶ側は動かない。

pub mod gallerydl;
pub mod invocation;
pub mod route;
pub mod tools;
pub mod whisper;
pub mod ytdlp;

use std::path::Path;

use thiserror::Error;

use crate::asset::Asset;
use crate::content::{Content, ContentType};
use crate::liveness::Presence;
use crate::source::Source;
use crate::timestamp::Timestamp;
use crate::url::NormalizedUrl;

pub use invocation::{Completed, Invocation, run};
pub use route::{Acquirer, Adapters, Discoverer, Prober};
pub use tools::{Resolution, Resolver, Tool};

/// 外部ツールが期待どおりに働かなかったとき。
///
/// **壊れ方を混ぜない。** #5 の生存確認は「分類ではなく非対称性」で決まるので、
/// 「消えた」と「分からない」を区別できることが要件になる。
#[derive(Debug, Error)]
pub enum AdapterError {
    /// ツールを起動できなかった。
    #[error("{program} を起動できない")]
    Launch {
        program: String,
        #[source]
        source: std::io::Error,
    },
    /// 出力が読めなかった。**ツールの仕様が変わった疑い**。
    ///
    /// 項目の問題ではないので、`error` 状態にせず人に知らせる。
    #[error("{program} の出力を読めない: {detail}")]
    Parse { program: String, detail: String },
    /// 配信元から消えたと断定できた。
    #[error("配信元に無い: {url}")]
    Unavailable { url: String },
    /// cookie を使えない。
    ///
    /// 失効しているか、設定した取り出し元のブラウザが入っていないか。後者だと
    /// **認証の要らない公開のものまで落ちる**ので、区別せず「設定を確かめる」へ
    /// 導く。どちらも個別の項目ではなくプラットフォーム全体の問題なので、
    /// `error` を並べず取得を止めて人に知らせる（#5）。
    #[error("cookie を使えない（設定の auth.cookies_from を確かめる）: {detail}")]
    Unauthenticated { detail: String },
    /// 一時的な失敗。次の回でやり直す。
    #[error("一時的に失敗した: {detail}")]
    Transient { program: String, detail: String },
}

impl AdapterError {
    /// 生存確認から見たときの観測。
    ///
    /// 「消えた」と断定できたときだけ [`Presence::Gone`]。それ以外は判定保留で、
    /// `holding` のまま次の回へ回る（#5）。
    pub const fn presence(&self) -> Presence {
        match self {
            Self::Unavailable { .. } => Presence::Gone,
            Self::Launch { .. }
            | Self::Parse { .. }
            | Self::Unauthenticated { .. }
            | Self::Transient { .. } => Presence::Unknown,
        }
    }
}

/// 検知で見つけた1件。
///
/// `Item` にする前の形。`id` はまだ無く、状態も決まっていない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub url: NormalizedUrl,
    pub published_at: Timestamp,
    pub content: Content,
    /// 取得する実体があるか。無ければ [`crate::state::State::initial`] で
    /// そのまま `kept` になる（#1）。
    pub media: crate::state::MediaPresence,
}

impl Found {
    pub fn content_type(&self) -> ContentType {
        self.content.content_type()
    }
}

/// 配信元から、まだ見ていない項目を見つける。
pub trait Discover {
    /// `since` 以降の項目を挙げる。
    ///
    /// #1 のとおり監視対象は `Source.created_at` 以降なので、それより古いものは
    /// 返さない。既に台帳に在るかの判定は呼ぶ側（`Item.url` の一意キー）。
    fn discover(
        &self,
        source: &Source,
        since: Timestamp,
    ) -> impl Future<Output = Result<Vec<Found>, AdapterError>> + Send;
}

/// 項目の実体を手元へ落とす。
pub trait Acquire {
    /// `into` へ書き、置けたものを返す。
    ///
    /// ファイル名の規則は #1 の `<kind>.<ordinal>.<ext>`。ライブが途中で切れれば
    /// 通し番号が増える。
    fn acquire(
        &self,
        url: &NormalizedUrl,
        content_type: ContentType,
        into: &Path,
    ) -> impl Future<Output = Result<Vec<Asset>, AdapterError>> + Send;
}

/// 手元の実体を文字起こしする。
pub trait Transcribe {
    /// 断片ごとに1本作る。断片間の空白時間が分からないので、通しのタイムスタンプに
    /// 繋げられないため（#1）。
    fn transcribe(
        &self,
        media: &Path,
        into: &Path,
        ordinal: std::num::NonZeroU32,
    ) -> impl Future<Output = Result<Asset, AdapterError>> + Send;
}

/// 参照でも同じ口を満たす。
///
/// [`Discover`] / [`Acquire`] / [`Probe`] は #5 の対応表が借りた値を包んだ enum を
/// 返すのに対し、文字起こしは種別で分かれないので道具そのものを借りて返す。
/// この1本があると、[`Ports`] の4つの口の形が揃う。
impl<T: Transcribe + Sync> Transcribe for &T {
    fn transcribe(
        &self,
        media: &Path,
        into: &Path,
        ordinal: std::num::NonZeroU32,
    ) -> impl Future<Output = Result<Asset, AdapterError>> + Send {
        (**self).transcribe(media, into, ordinal)
    }
}

/// 配信元にまだ在るかを確かめる。
pub trait Probe {
    /// #5 のとおり、**在ることを確かめられたときだけ** [`Presence::Present`]。
    /// 分からなければ [`Presence::Unknown`] で、判定は次の回へ回る。
    fn probe(
        &self,
        url: &NormalizedUrl,
    ) -> impl Future<Output = Result<Presence, AdapterError>> + Send;
}

/// エンジンが外の世界へ触れる口を、ひとまとめにしたもの。
///
/// エンジン（[`crate::engine`]）は #5 の対応表を知らない。「この配信元を検知する
/// もの」「この種別を取るもの」を訊くだけで、**どのツールが動いたかを見ない**。
/// 実物は [`Adapters`]、テストは口だけを満たした偽物を差す。
///
/// 4つの口を1つの trait に束ねるのは、エンジンが4つ全部を同時に要るから。
/// 別々の型引数にすると、呼ぶ側が対応表と同じ組み合わせを手で揃えることになり、
/// 表を1か所に置いた意味が消える。
pub trait Ports {
    /// #5 の「検知」列。プラットフォームを決められない配信元はここで落ちる。
    fn discoverer(&self, source: &Source) -> Result<impl Discover, AdapterError>;

    /// #5 の「取得」列。全種別に担当がいるので、選ぶところでは失敗しない。
    fn acquirer(&self, content_type: ContentType) -> impl Acquire;

    /// #5 の「生存確認」列。手段が決まっていない種別にも
    /// [`Presence::Unknown`] を返すものが返るので、選ぶところでは失敗しない。
    fn prober(&self, content_type: ContentType) -> impl Probe;

    /// 文字起こし。#5 の表に列が無い — 種別で分かれないため。
    fn transcriber(&self) -> impl Transcribe;
}

#[cfg(test)]
mod tests {
    use crate::config::Browser;

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

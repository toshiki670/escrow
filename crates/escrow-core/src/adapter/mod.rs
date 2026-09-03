//! 外部アクセスの語彙。
//!
//! 「配信元から、まだ見ていない項目を見つける」「項目の実体を手元へ落とす」を
//! trait として定め、実際にどのツールがそれを行うかは知らない。実装は
//! `escrow-adapter`、呼ぶ順序と時刻は `escrow-scheduler`（#3）。
//!
//! # trait はドメインの語彙で切る
//!
//! [`Discover`] / [`Acquire`] / [`Transcribe`] / [`Probe`] のどれにも、ツール固有の
//! 語を出さない。「タイムラインを列挙する」ではなく「配信元から、まだ見ていない
//! 項目を見つける」。ツールを入れ替えても、呼ぶ側は動かない。

pub mod tools;

use std::path::Path;

use thiserror::Error;

use crate::asset::Asset;
use crate::content::{Content, ContentType};
use crate::liveness::Presence;
use crate::source::Source;
use crate::timestamp::Timestamp;
use crate::url::NormalizedUrl;

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

/// 配信元にまだ在るかを確かめる。
pub trait Probe {
    /// #5 のとおり、**在ることを確かめられたときだけ** [`Presence::Present`]。
    /// 分からなければ [`Presence::Unknown`] で、判定は次の回へ回る。
    fn probe(
        &self,
        url: &NormalizedUrl,
    ) -> impl Future<Output = Result<Presence, AdapterError>> + Send;
}

//! 外部ツールの呼び出しと、その語彙。
//!
//! 「配信元から、まだ見ていない項目を見つける」「項目の実体を手元へ落とす」を
//! trait として定め、それを実際のプロセス起動と HTTP へ落とす。#5 が決めた対応表の
//! 実装にあたる層で、#5 自身が「ここで決めるのは実装の詳細で、データモデル（#1）より
//! 可変性が高い」と言っているとおり、**ここは変わる前提**で組む。X の仕様変更、
//! ツールのフラグ変更、ツールそのものの入れ替えに耐えること。
//!
//! **この crate を依存に持つのは `escrow-scheduler` だけ**（#3）。スライスも UI も
//! 外部ツールを名前で知らないので、外部アクセスの迂回はコンパイルエラーになる。
//! スライスが書くのは `escrow_scheduler::` から始まる名前だけで、ここの trait と
//! 型はそちらが再公開したものを通って届く（#15）。
//!
//! # trait はドメインの語彙で切る
//!
//! [`Discover`] / [`Acquire`] / [`Transcribe`] / [`Probe`] のどれにも、ツール固有の
//! 語を出さない。「タイムラインを列挙する」ではなく「配信元から、まだ見ていない
//! 項目を見つける」。ツールを入れ替えても、呼ぶ側は動かない。
//!
//! # 外へ出る点は受付を通る
//!
//! ツールを叩くメソッドも、引数を組み立てる関数も、プロセスを起動する `run` も、
//! crate の中でだけ見える。**外から呼べるのは [`route::Adapters`] が返す包みだけ**で、
//! その中で [`through`] が順番を待つ（#13）。予算を迂回する呼び出しは、この crate の
//! 外では綴れない。
//!
//! # 分け方
//!
//! 3つの層を混ぜない。それぞれ別の理由で壊れるため。
//!
//! | 層 | 形 | 壊れる理由 |
//! |---|---|---|
//! | 引数の組み立て | 純関数 → `Invocation` | ツールのフラグが変わった |
//! | 出力の読み取り | 純関数 `&str -> Result<_, AdapterError>` | ツールの出力形式が変わった |
//! | 実行 | `run` 1か所 | OS 側の事情 |
//!
//! 1つの関数に混ぜると、落ちたときにどれが原因か分からない。分けてあれば、
//! 引数のテストはプロセスを起動せず argv を突き合わせるだけで済み、出力のテストは
//! 実物を固めた fixture で offline に回せる。

pub mod gallerydl;
pub(crate) mod invocation;
pub mod route;
pub mod rss;
pub mod whisper;
pub mod ytdlp;

pub use route::{Acquirer, Adapters, Discoverer, Prober};

use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use thiserror::Error;

use escrow_domain::asset::Asset;
use escrow_domain::content::{Content, ContentType};
use escrow_domain::liveness::Presence;
use escrow_domain::source::Source;
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::NormalizedUrl;

/// [`Discover`] / [`Acquire`] / [`Transcribe`] / [`Probe`] / [`Admit`] が返す future。
///
/// **`Box` へ入れると、これらを `dyn` にできる**（#7）。`impl Future` を返す形は
/// `dyn` にできず、型引数が呼ぶ側へ伝播して入口まで届く。`Box` にすれば入口の型が
/// 具象のままになり、払うのは呼び出し1回あたりの確保1回だけ — 相手はプロセス起動か
/// HTTP なので、その1回は測れる大きさに届かない。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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
    #[error("{program} が一時的に失敗した: {detail}")]
    Transient { program: String, detail: String },
    /// 断られた。
    ///
    /// **一時的な失敗と分けて持つ**（#13）。分けずに扱うと、断られた直後に同じ速さで
    /// 叩き直し、やり直しが止められている原因そのものになる。
    #[error("{program} に断られた: {detail}")]
    Rejected {
        program: String,
        detail: String,
        /// 相手が指定した待ち時間。無ければ #2 の `schedule.rejection_backoff_seconds`。
        retry_after: Option<Duration>,
    },
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
            | Self::Transient { .. }
            | Self::Rejected { .. } => Presence::Unknown,
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
    /// 配信の開始予定時刻。予約枠でなければ空（#1）。
    ///
    /// 始まってしまった配信には入らない。#13 がこの時刻に取得を予約するので、
    /// 過ぎた時刻を入れると待つ意味が無くなる。
    pub scheduled_start_at: Option<Timestamp>,
    pub content: Content,
    /// 取得する実体があるか。無ければ [`escrow_domain::state::State::initial`] で
    /// そのまま `kept` になる（#1）。
    pub media: escrow_domain::state::MediaPresence,
}

impl Found {
    pub fn content_type(&self) -> ContentType {
        self.content.content_type()
    }
}

/// 外へ出る要求の種類。#13 の経路がそのまま並ぶ。
///
/// 文字起こしがここに無いのは、whisper がローカルで動いて外へ要求を出さないため。
/// 一覧に無ければ、飛ばす理由を書く場所も要らない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Route {
    /// 配信元を1回読む。
    Discover,
    /// 1件のメタデータを取る。予約枠のポーリングもここを通る。
    Describe,
    /// 実体を手元へ落とす。
    Acquire,
    /// 配信元にまだ在るかを確かめる。
    Probe,
}

impl Route {
    pub const ALL: [Self; 4] = [Self::Discover, Self::Describe, Self::Acquire, Self::Probe];
}

/// 外へ出る順番を決めるもの。
///
/// **実装は `escrow-scheduler`**（#13）。ここが宣言するのは「外へ出る点はどこか」
/// までで、上限の値も待ち方も順番の決め方も持たない。
pub trait Admit: Send + Sync {
    /// この経路の順番が来るまで待つ。
    fn admit(&self, route: Route) -> BoxFuture<'_, Box<dyn Permit + '_>>;
}

/// 順番が回ってきたことの証。**落とすと枠が空く。**
pub trait Permit: Send {
    /// 呼び出しがどうなったかを伝える。断られていれば、その経路がしばらく閉じる。
    fn report(&self, result: Result<(), &AdapterError>);
}

/// 順番を待ってから外へ出て、結果を伝える。
///
/// **この crate が外へ出る点は、すべてここを通る**（#13）。素通りさせると、その1本ぶん
/// だけ予算が実際より軽く見える。
///
/// # Errors
///
/// `call` が返した失敗をそのまま返す。順番待ちそのものは失敗しない。
pub async fn through<T>(
    admit: &dyn Admit,
    route: Route,
    call: impl Future<Output = Result<T, AdapterError>>,
) -> Result<T, AdapterError> {
    let permit = admit.admit(route).await;
    let result = call.await;
    permit.report(result.as_ref().map(|_| ()));
    result
}

/// 配信元から、まだ見ていない項目を見つける。
pub trait Discover {
    /// `since` 以降の項目を挙げる。
    ///
    /// #1 のとおり監視対象は `Source.created_at` 以降なので、それより古いものは
    /// 返さない。既に台帳に在るかの判定は呼ぶ側（`Item.url` の一意キー）。
    fn discover<'a>(
        &'a self,
        source: &'a Source,
        since: Timestamp,
    ) -> BoxFuture<'a, Result<Vec<Found>, AdapterError>>;
}

/// 項目の実体を手元へ落とす。
pub trait Acquire {
    /// `into` へ書き、置けたものを返す。
    ///
    /// ファイル名の規則は #1 の `<kind>.<ordinal>.<ext>`。ライブが途中で切れれば
    /// 通し番号が増える。
    ///
    /// 種別は受け取らない。**どのツールが取るかは呼ぶ前に決まっている**ので、
    /// ここへ渡しても決めることが残っていない。
    fn acquire<'a>(
        &'a self,
        url: &'a NormalizedUrl,
        into: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<Asset>, AdapterError>>;
}

/// 手元の実体を文字起こしする。
pub trait Transcribe {
    /// 断片ごとに1本作る。断片間の空白時間が分からないので、通しのタイムスタンプに
    /// 繋げられないため（#1）。
    fn transcribe<'a>(
        &'a self,
        media: &'a Path,
        into: &'a Path,
        ordinal: std::num::NonZeroU32,
    ) -> BoxFuture<'a, Result<Asset, AdapterError>>;
}

/// 配信元にまだ在るかを確かめる。
pub trait Probe {
    /// #5 のとおり、**在ることを確かめられたときだけ** [`Presence::Present`]。
    /// 分からなければ [`Presence::Unknown`] で、判定は次の回へ回る。
    fn probe<'a>(&'a self, url: &'a NormalizedUrl)
    -> BoxFuture<'a, Result<Presence, AdapterError>>;
}

#[cfg(test)]
mod tests {
    use escrow_config::Browser;

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

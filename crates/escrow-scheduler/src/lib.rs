//! 外部アクセスの一元化（#13）。
//!
//! 外へ出る呼び出しはすべてここを通る。**迂回できないことを守っているのは依存の向き**
//! で、正本は `tests/dependency_direction.rs`。
//!
//! # この crate の公開 API が port そのもの（#15）
//!
//! 別に trait を切らず、外部アクセスの語彙をここから再公開する。スライスが書く
//! 名前は `escrow_scheduler::` から始まり、**`escrow-external` の型を1つも名前で
//! 知らない**。テストで差し替えるときも、この再公開した trait を実装すればよい。
//!
//! # 語彙は3つ（#13）
//!
//! | 向き | 語彙 | 型 |
//! |---|---|---|
//! | 要求する側 → | 何が欲しいか | 下の trait のどれかを呼ぶ |
//! | 要求する側 → | いつまでに要るか | [`Demand`] |
//! | ← スケジューラ | いつ出すつもりか | [`Plan`] |
//!
//! 返ってきたものを呼ぶと、**その中で順番を待つ。**
//!
//! # 予算は要求を出す側の外に在る
//!
//! 各経路が自分で自分を抑える形だと、経路が複数ある時点で合計を誰も知らない（#13）。
//! [`budget::Budget`] が経路ごとの門を持ち、`escrow-external` の外向きの呼び出しは
//! 1本残らずそこを通る。
//!

pub mod budget;

use std::path::{Path, PathBuf};

use thiserror::Error;

use escrow_config::{Config, Paths, Resolver, Tool};
use escrow_domain::asset::Asset;
use escrow_domain::content::{ContentType, Platform};
use escrow_domain::liveness::Presence;
use escrow_domain::source::Source;
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::{self, NormalizedUrl};
use escrow_external::gallerydl::GalleryDl;
use escrow_external::route::Adapters;
use escrow_external::rss::Rss;
use escrow_external::whisper::Whisper;
use escrow_external::ytdlp::YtDlp;

/// 外部アクセスの語彙。**スライスが触れてよい外の世界はこれで全部**（#15）。
///
/// `escrow-external` で定義されたものを、そのままここから見せる。写しを作らないので
/// 型は1つきりで、変換の層も要らない。
pub use budget::{Demand, Next, Plan};
pub use escrow_external::{
    Acquire, AdapterError, BoxFuture, Discover, Found, Probe, Route, Transcribe,
};

use budget::{Budget, Turn};

/// 要るツールが見つからなかった。
///
/// どれが無いかだけを持ち、人への案内は呼ぶ側が足す。
#[derive(Debug, Error)]
#[error("{0} が見つからない")]
pub struct MissingTool(pub Tool);

/// 外部アクセスの受付。
pub struct Scheduler {
    adapters: Adapters,
    whisper: Whisper,
    budget: Budget,
}

impl Scheduler {
    /// 設定と解決済みのツールから組み立てる。
    ///
    /// **どのツールが要るかを知っているのはここだけ。** 呼ぶ側は解決器（#2）を
    /// 渡すだけで、名前の一覧を持たない。
    pub fn new(config: &Config, paths: &Paths, resolver: &Resolver) -> Result<Self, MissingTool> {
        let browser = config.auth.cookies_from;

        Ok(Self {
            budget: Budget::new(&config.schedule),
            adapters: Adapters::new(
                Rss::new(),
                YtDlp::new(path(resolver, Tool::YtDlp)?, browser),
                GalleryDl::new(path(resolver, Tool::GalleryDl)?, browser),
            ),
            whisper: Whisper::new(
                path(resolver, Tool::WhisperCli)?,
                path(resolver, Tool::Ffmpeg)?,
                &paths.transcribe_model,
                config.transcribe.language.clone(),
            ),
        })
    }

    /// 1件のメタデータを取る。
    ///
    /// # Errors
    ///
    /// 順番を待ったうえで外へ出て、そこで起きたことをそのまま返す。
    pub async fn describe(
        &self,
        url: &NormalizedUrl,
        content_type: ContentType,
        demand: Demand,
    ) -> Result<Found, AdapterError> {
        let turn = self.budget.turn(content_type.platform(), demand);

        self.adapters.describe(url, content_type, &turn).await
    }

    /// この配信元を見るもの。#5 の対応表が決める。
    ///
    /// # Errors
    ///
    /// 配信元の URL からプラットフォームを決められないとき。
    pub fn discoverer(
        &self,
        source: &Source,
        demand: Demand,
    ) -> Result<Box<dyn Discover + '_>, AdapterError> {
        let platform = url::platform_of_source(&source.url).ok_or_else(|| AdapterError::Parse {
            program: "escrow".to_owned(),
            detail: format!("どのプラットフォームの配信元か決められない: {}", source.url),
        })?;

        Ok(Box::new(Discovering {
            adapters: &self.adapters,
            turn: self.budget.turn(platform, demand),
            platform,
        }))
    }

    /// この種別を取るもの。#5 の対応表が決める。
    pub fn acquirer(&self, content_type: ContentType, demand: Demand) -> Box<dyn Acquire + '_> {
        Box::new(Acquiring {
            adapters: &self.adapters,
            turn: self.budget.turn(content_type.platform(), demand),
            content_type,
        })
    }

    /// この種別の生存確認をするもの。#5 の対応表が決める。
    ///
    /// **手段を決めていない種別では空**（X 投稿）。空を返すので、**確かめようのない
    /// ものに予算を使わない**。
    pub fn prober(&self, content_type: ContentType, demand: Demand) -> Option<Box<dyn Probe + '_>> {
        Adapters::has_prober(content_type).then(|| {
            Box::new(Probing {
                adapters: &self.adapters,
                turn: self.budget.turn(content_type.platform(), demand),
                content_type,
            }) as Box<dyn Probe + '_>
        })
    }

    /// 文字起こしをするもの。
    ///
    /// **予算を通らない。** whisper はローカルで動き、外へ要求を出さない（#13）。
    pub const fn transcriber(&self) -> &dyn Transcribe {
        &self.whisper
    }

    /// いつ出すつもりかを、経路ごとに答える（#13）。UI がこれを読む。
    ///
    /// `now` は答えを壁の時計へ写すための起点（[`budget`] の「時計が2つある」）。
    pub fn plan(&self, now: Timestamp) -> Vec<Plan> {
        self.budget.plan(now)
    }
}

/// 予算を通してから配信元を見る。
struct Discovering<'a> {
    adapters: &'a Adapters,
    turn: Turn<'a>,
    platform: Platform,
}

impl Discover for Discovering<'_> {
    fn discover<'a>(
        &'a self,
        source: &'a Source,
        since: Timestamp,
    ) -> BoxFuture<'a, Result<Vec<Found>, AdapterError>> {
        Box::pin(async move {
            self.adapters
                .discoverer(self.platform, &self.turn)
                .discover(source, since)
                .await
        })
    }
}

/// 予算を通してから実体を落とす。
struct Acquiring<'a> {
    adapters: &'a Adapters,
    turn: Turn<'a>,
    content_type: ContentType,
}

impl Acquire for Acquiring<'_> {
    fn acquire<'a>(
        &'a self,
        url: &'a NormalizedUrl,
        into: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<Asset>, AdapterError>> {
        Box::pin(async move {
            self.adapters
                .acquirer(self.content_type, &self.turn)
                .acquire(url, into)
                .await
        })
    }
}

/// 予算を通してから配信元を確かめる。
struct Probing<'a> {
    adapters: &'a Adapters,
    turn: Turn<'a>,
    content_type: ContentType,
}

impl Probe for Probing<'_> {
    fn probe<'a>(
        &'a self,
        url: &'a NormalizedUrl,
    ) -> BoxFuture<'a, Result<Presence, AdapterError>> {
        Box::pin(self.adapters.probe(url, self.content_type, &self.turn))
    }
}

fn path(resolver: &Resolver, tool: Tool) -> Result<PathBuf, MissingTool> {
    resolver
        .resolve(tool)
        .path()
        .map(std::path::Path::to_path_buf)
        .ok_or(MissingTool(tool))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::num::NonZeroU32;
    use std::time::Duration;

    use tokio::time::Instant;

    use escrow_config::{Browser, Schedule};
    use escrow_external::gallerydl::GalleryDl;
    use escrow_external::rss::Rss;
    use escrow_external::ytdlp::YtDlp;

    /// 起動できないパスを渡す。**外へは出ない** — 予算が掛かるのは起動より前なので、
    /// 待ちの有無はツールが動くかどうかと関係しない。
    fn adapters() -> Adapters {
        Adapters::new(
            Rss::new(),
            YtDlp::new("/nonexistent/yt-dlp", Browser::Firefox),
            GalleryDl::new("/nonexistent/gallery-dl", Browser::Firefox),
        )
    }

    /// 予算が、包みを通った実際の呼び出しに掛かること。
    ///
    /// 門そのものの振る舞いは [`budget`] の単体テストが見る。ここが見るのは**繋がり** —
    /// [`Turn`] → `Admit` → `through` → 包み → ツール が1本になっていること。
    #[tokio::test(start_paused = true)]
    async fn the_budget_applies_to_a_call_that_goes_through_the_adapters() {
        let adapters = adapters();
        let budget = Budget::new(&Schedule::default());
        let url = url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
            .unwrap()
            .0;
        let demand = Demand::weighed(NonZeroU32::MIN);

        let describe = async |budget: &Budget| {
            let turn = budget.turn(Platform::Youtube, demand);
            adapters
                .describe(&url, ContentType::YoutubeVideo, &turn)
                .await
        };

        let start = Instant::now();
        assert!(describe(&budget).await.is_err(), "起動できない");
        assert_eq!(Instant::now() - start, Duration::ZERO, "1本目は待たない");

        assert!(describe(&budget).await.is_err());
        assert_eq!(
            Instant::now() - start,
            Duration::from_secs(10),
            "2本目は #2 の describe_gap_seconds ぶん待つ"
        );
    }
}

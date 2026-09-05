//! 外部アクセスの一元化（#13）。
//!
//! **`escrow-external` を依存に持つ唯一の crate**（#3）。外へ出る呼び出しはすべて
//! ここを通り、迂回できないことは依存の向きが保証する。
//!
//! # この crate の公開 API が port そのもの（#15）
//!
//! 別に trait を切らず、外部アクセスの語彙をここから再公開する。スライスが書く
//! 名前は `escrow_scheduler::` から始まり、**`escrow-external` の型を1つも名前で
//! 知らない**。テストで差し替えるときも、この再公開した trait を実装すればよい。
//!
//! # いまは通すだけ
//!
//! #13 の4概念 — 受付・予算・拒否と再試行・順序 — は Phase 5 で入る。この段階に
//! 在るのは、アダプタを揃えて呼び出しをそのまま渡す層だけ。**順序も待機もまだ無い。**

use std::path::PathBuf;

use thiserror::Error;

use escrow_config::{Config, Paths, Resolver, Tool};
use escrow_domain::content::ContentType;
use escrow_domain::url::NormalizedUrl;
use escrow_external::gallerydl::GalleryDl;
use escrow_external::route::Adapters;
use escrow_external::rss::Rss;
use escrow_external::whisper::Whisper;
use escrow_external::ytdlp::YtDlp;

/// 外部アクセスの語彙。**スライスが触れてよい外の世界はこれで全部**（#15）。
///
/// `escrow-external` で定義されたものを、そのままここから見せる。写しを作らないので
/// 型は1つきりで、変換の層も要らない。
pub use escrow_external::{Acquire, AdapterError, Discover, Found, Probe, Transcribe};

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
}

impl Scheduler {
    /// 設定と解決済みのツールから組み立てる。
    ///
    /// **どのツールが要るかを知っているのはここだけ。** 呼ぶ側は解決器（#2）を
    /// 渡すだけで、名前の一覧を持たない。
    pub fn new(config: &Config, paths: &Paths, resolver: &Resolver) -> Result<Self, MissingTool> {
        let browser = config.auth.cookies_from;

        Ok(Self {
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
    pub async fn describe(
        &self,
        url: &NormalizedUrl,
        content_type: ContentType,
    ) -> Result<Found, AdapterError> {
        self.adapters.describe(url, content_type).await
    }

    /// この種別を取るもの。#5 の対応表が決める。
    pub fn acquirer(&self, content_type: ContentType) -> impl Acquire + use<'_> {
        self.adapters.acquirer(content_type)
    }

    /// 文字起こしをするもの。
    pub const fn transcriber(&self) -> &impl Transcribe {
        &self.whisper
    }
}

fn path(resolver: &Resolver, tool: Tool) -> Result<PathBuf, MissingTool> {
    resolver
        .resolve(tool)
        .path()
        .map(std::path::Path::to_path_buf)
        .ok_or(MissingTool(tool))
}

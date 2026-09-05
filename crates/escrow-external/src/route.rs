//! #5 の対応表。**プラットフォーム × 操作 → ツール**。
//!
//! | | 検知 | 取得 | 生存確認 |
//! |---|---|---|---|
//! | YouTube ショート / 動画 / 配信 | RSS ＋ yt-dlp の追加取得 | yt-dlp | yt-dlp |
//! | X 投稿 | gallery-dl | gallery-dl | **未決**（#5） |
//! | X Space | gallery-dl | yt-dlp | yt-dlp |
//! | X ライブ配信 | gallery-dl | yt-dlp | yt-dlp |
//!
//! **この対応をコードの1か所に置く。** 散らばっていると「X の取得を別ツールへ」が
//! 何箇所の変更になるか分からない。ここに集めてあれば、変わるのは表の1行になる。
//!
//! 呼ぶ側（パイプライン・CLI・エンジン）はどのツールが動いたかを知らない。
//!
//! # 外へ出る点はここに集まる
//!
//! 下の [`Acquirer`] / [`Discoverer`] / [`Prober`] / [`Adapters::describe`] が、
//! この crate の外向きの呼び出しを1本残らず [`through`] へ通す（#13）。ツールの側は
//! 素のメソッドを持つだけなので、予算を迂回する経路が crate の外から見えない。

use std::path::Path;

use crate::gallerydl::GalleryDl;
use crate::rss::Rss;
use crate::ytdlp::YtDlp;
use crate::{Acquire, AdapterError, Admit, BoxFuture, Discover, Found, Probe, Route, through};
use escrow_domain::asset::Asset;
use escrow_domain::content::{Content, ContentType, Platform};
use escrow_domain::liveness::Presence;
use escrow_domain::source::Source;
use escrow_domain::state::MediaPresence;
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::{NormalizedUrl, TypeHint};

/// 使えるツールを揃えたもの。
///
/// **中のツールを外へ見せない。** 見せると呼ぶ側が対応表を迂回して
/// `adapters.ytdlp` を直に叩けてしまい、「この対応をコードの1か所に置く」が
/// 約束にしかならない。外から呼べるのは下の3つだけ。
pub struct Adapters {
    rss: Rss,
    ytdlp: YtDlp,
    gallerydl: GalleryDl,
}

impl Adapters {
    pub const fn new(rss: Rss, ytdlp: YtDlp, gallerydl: GalleryDl) -> Self {
        Self {
            rss,
            ytdlp,
            gallerydl,
        }
    }

    /// 対応表の「取得」列。
    ///
    /// X の Space とライブ配信が gallery-dl でないのは、`twitter:spaces` /
    /// `twitter:broadcast` を持つのが yt-dlp だけだから（#5）。
    pub const fn acquirer<'a>(
        &'a self,
        content_type: ContentType,
        admit: &'a dyn Admit,
    ) -> Acquirer<'a> {
        let tool = match content_type {
            ContentType::YoutubeShorts | ContentType::YoutubeVideo | ContentType::YoutubeLive => {
                AcquireTool::YtDlp(&self.ytdlp)
            }
            ContentType::XPost => AcquireTool::GalleryDl(&self.gallerydl),
            ContentType::XSpace | ContentType::XBroadcast => AcquireTool::YtDlp(&self.ytdlp),
        };
        Acquirer { tool, admit }
    }

    /// 対応表の「検知」列。
    ///
    /// YouTube は RSS。認証が要らず、1チャンネル1回で済み、叩いてよい頻度が
    /// 公表されている（#5）。X は yt-dlp ではタイムラインを列挙できないので
    /// gallery-dl。
    pub const fn discoverer<'a>(
        &'a self,
        platform: Platform,
        admit: &'a dyn Admit,
    ) -> Discoverer<'a> {
        let tool = match platform {
            Platform::Youtube => DiscoverTool::Youtube {
                rss: &self.rss,
                ytdlp: &self.ytdlp,
            },
            Platform::X => DiscoverTool::GalleryDl(&self.gallerydl),
        };
        Discoverer { tool, admit }
    }

    /// 対応表の「生存確認」列。
    ///
    /// **X 投稿には担い手がいない** — #5 が手段を決めていない。#5 の非対称性により
    /// 確かめられない項目は `holding` に留まるので、空を返すことがそのまま仕様になる。
    pub const fn prober<'a>(
        &'a self,
        content_type: ContentType,
        admit: &'a dyn Admit,
    ) -> Option<Prober<'a>> {
        if Self::has_prober(content_type) {
            Some(Prober {
                tool: &self.ytdlp,
                admit,
            })
        } else {
            None
        }
    }

    /// 生存確認の担い手がいる種別か。
    ///
    /// 担い手そのものを組み立てずに済むので、**[`Admit`] を持たない場所からも対応表へ
    /// 訊ける**。[`Adapters::prober`] もここを読むので、答えは1か所で決まる。
    pub const fn has_prober(content_type: ContentType) -> bool {
        match content_type {
            ContentType::XPost => false,
            ContentType::YoutubeShorts
            | ContentType::YoutubeVideo
            | ContentType::YoutubeLive
            | ContentType::XSpace
            | ContentType::XBroadcast => true,
        }
    }

    /// 1件が配信元に在るかを確かめる。
    ///
    /// **担い手がいない種別は、観測できなかったとして返す**（#5）。#5 の非対称性が
    /// 「それ以外すべて」を判定保留に寄せているので、手段を決めていないことも
    /// その中に収まる。
    pub async fn probe(
        &self,
        url: &NormalizedUrl,
        content_type: ContentType,
        admit: &dyn Admit,
    ) -> Result<Presence, AdapterError> {
        match self.prober(content_type, admit) {
            Some(prober) => prober.probe(url).await,
            None => Ok(Presence::Unknown),
        }
    }

    /// 1件のメタデータを取る。
    ///
    /// 分かれ目は subtype。`Media` は yt-dlp、`Post` は gallery-dl。本文と
    /// 繋がりの URL を返せるのが gallery-dl だけだから（#5）。
    pub async fn describe(
        &self,
        url: &NormalizedUrl,
        content_type: ContentType,
        admit: &dyn Admit,
    ) -> Result<Found, AdapterError> {
        match content_type.media_type() {
            Some(media_type) => {
                through(admit, Route::Describe, self.ytdlp.describe(url, media_type)).await
            }
            None => through(admit, Route::Describe, self.gallerydl.describe(url)).await,
        }
    }
}

/// 取得を担うもの。呼ぶ側はどれが動いたかを知らない。
pub struct Acquirer<'a> {
    tool: AcquireTool<'a>,
    admit: &'a dyn Admit,
}

enum AcquireTool<'a> {
    YtDlp(&'a YtDlp),
    GalleryDl(&'a GalleryDl),
}

impl Acquire for Acquirer<'_> {
    fn acquire<'a>(
        &'a self,
        url: &'a NormalizedUrl,
        into: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<Asset>, AdapterError>> {
        Box::pin(async move {
            let admit = self.admit;
            match self.tool {
                AcquireTool::YtDlp(tool) => {
                    through(admit, Route::Acquire, tool.acquire(url, into)).await
                }
                AcquireTool::GalleryDl(tool) => {
                    through(admit, Route::Acquire, tool.acquire(url, into)).await
                }
            }
        })
    }
}

/// 生存確認を担うもの。
pub struct Prober<'a> {
    tool: &'a YtDlp,
    admit: &'a dyn Admit,
}

impl Probe for Prober<'_> {
    fn probe<'a>(
        &'a self,
        url: &'a NormalizedUrl,
    ) -> BoxFuture<'a, Result<Presence, AdapterError>> {
        Box::pin(through(self.admit, Route::Probe, self.tool.probe(url)))
    }
}

/// 検知を担うもの。
pub struct Discoverer<'a> {
    tool: DiscoverTool<'a>,
    admit: &'a dyn Admit,
}

enum DiscoverTool<'a> {
    /// **RSS で見つけ、足りないぶんだけ yt-dlp で埋める**（#5）。
    ///
    /// フィードは1チャンネル1回で15件を返すが、`/watch?v=` の項目が動画か配信かを
    /// 語らず、開始時刻も持たない。そこだけを1件ごとの追加取得で埋める。
    Youtube {
        rss: &'a Rss,
        ytdlp: &'a YtDlp,
    },
    GalleryDl(&'a GalleryDl),
}

impl Discover for Discoverer<'_> {
    fn discover<'a>(
        &'a self,
        source: &'a Source,
        since: Timestamp,
    ) -> BoxFuture<'a, Result<Vec<Found>, AdapterError>> {
        Box::pin(async move {
            match self.tool {
                DiscoverTool::Youtube { rss, ytdlp } => {
                    discover_youtube(rss, ytdlp, source, since, self.admit).await
                }
                DiscoverTool::GalleryDl(tool) => {
                    through(self.admit, Route::Discover, tool.discover(source, since)).await
                }
            }
        })
    }
}

/// フィードを読み、`since` 以降のものだけ追加取得へ回す。
///
/// **追加取得の回数を決めるのは `since`。** フィードは常に15件返すので、絞らないと
/// 巡回のたびに15回叩くことになる。台帳との突き合わせは呼ぶ側の仕事なので（#1 の
/// `Item.url` の一意キー）、ここでは日時だけで切る。
///
/// 外へ出るのは1回ではない。フィードが1回、追加取得が項目ごとに1回で、**それぞれ別の
/// 経路の予算に載る**（#13）。まとめて1回として数えると、配信元1本ぶんの要求が実際の
/// 15分の1に見える。
async fn discover_youtube(
    rss: &Rss,
    ytdlp: &YtDlp,
    source: &Source,
    since: Timestamp,
    admit: &dyn Admit,
) -> Result<Vec<Found>, AdapterError> {
    let mut found = Vec::new();

    let sightings = through(admit, Route::Discover, rss.sightings(&source.url)).await?;

    for sighting in sightings {
        if sighting.published_at < since {
            continue;
        }

        // ショートはフィードだけで決まる。追加取得は要らない。
        let (media_type, scheduled_start_at) = match sighting.hint {
            TypeHint::Known(content_type) => (
                content_type
                    .media_type()
                    .ok_or_else(|| AdapterError::Parse {
                        program: "escrow".to_owned(),
                        detail: format!("YouTube に本文だけの種別は無い: {content_type}"),
                    })?,
                None,
            ),
            TypeHint::YoutubeUnknown => {
                let schedule =
                    through(admit, Route::Describe, ytdlp.schedule(&sighting.url)).await?;
                (schedule.media_type, schedule.scheduled_start_at)
            }
        };

        found.push(Found {
            url: sighting.url,
            // 枠を作った時刻。予約枠の開始予定時刻とは別物なので、追加取得の値で
            // 上書きしない（#5）。
            published_at: sighting.published_at,
            scheduled_start_at,
            content: Content::Media {
                media_type,
                title: sighting.title,
            },
            // YouTube のものは、どれも落とす実体を持つ。
            media: MediaPresence::Present,
        });
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_config::Browser;

    fn adapters() -> Adapters {
        Adapters::new(
            Rss::new(),
            YtDlp::new("/bin/yt-dlp", Browser::Firefox),
            GalleryDl::new("/bin/gallery-dl", Browser::Firefox),
        )
    }

    /// 順番を待たせないもの。対応表だけを見るテストで使う。
    struct Anytime;

    impl Admit for Anytime {
        fn admit(&self, _route: Route) -> BoxFuture<'_, Box<dyn crate::Permit + '_>> {
            Box::pin(async { Box::new(Self) as Box<dyn crate::Permit + '_> })
        }
    }

    impl crate::Permit for Anytime {
        fn report(&self, _result: Result<(), &AdapterError>) {}
    }

    fn acquirer_name(acquirer: &Acquirer<'_>) -> &'static str {
        match acquirer.tool {
            AcquireTool::YtDlp(_) => "yt-dlp",
            AcquireTool::GalleryDl(_) => "gallery-dl",
        }
    }

    fn discoverer_name(discoverer: &Discoverer<'_>) -> &'static str {
        match discoverer.tool {
            DiscoverTool::Youtube { .. } => "rss",
            DiscoverTool::GalleryDl(_) => "gallery-dl",
        }
    }

    /// #5 の対応表の「取得」列をそのまま写したもの。表が動いたらここが落ちる。
    #[test]
    fn acquisition_follows_the_table() {
        let adapters = adapters();
        let admit = Anytime;

        for (content_type, expected) in [
            (ContentType::YoutubeShorts, "yt-dlp"),
            (ContentType::YoutubeVideo, "yt-dlp"),
            (ContentType::YoutubeLive, "yt-dlp"),
            (ContentType::XPost, "gallery-dl"),
            // Space とライブ配信は X だが、extractor を持つのは yt-dlp だけ。
            (ContentType::XSpace, "yt-dlp"),
            (ContentType::XBroadcast, "yt-dlp"),
        ] {
            assert_eq!(
                acquirer_name(&adapters.acquirer(content_type, &admit)),
                expected,
                "{content_type}"
            );
        }
    }

    /// 「検知」列。YouTube は RSS、X は gallery-dl（#5）。
    #[test]
    fn discovery_follows_the_table() {
        let adapters = adapters();
        let admit = Anytime;

        assert_eq!(
            discoverer_name(&adapters.discoverer(Platform::Youtube, &admit)),
            "rss"
        );
        assert_eq!(
            discoverer_name(&adapters.discoverer(Platform::X, &admit)),
            "gallery-dl"
        );
    }

    /// 「生存確認」列。X 投稿だけ担い手がいない（#5）。
    #[test]
    fn liveness_checks_follow_the_table() {
        let adapters = adapters();
        let admit = Anytime;

        for content_type in ContentType::ALL {
            let expected = content_type != ContentType::XPost;
            assert_eq!(
                adapters.prober(content_type, &admit).is_some(),
                expected,
                "{content_type}"
            );
            assert_eq!(
                Adapters::has_prober(content_type),
                expected,
                "担い手の有無と、担い手そのものが食い違う: {content_type}"
            );
        }
    }

    /// 担い手がいない種別は、外へ出ずに判定保留を返す（#5）。
    ///
    /// アダプタのパスは存在しないので、**起動すれば失敗が返る**。`Ok` が返ることが
    /// そのまま「出ていない」の証拠になる。
    #[tokio::test]
    async fn a_type_with_no_prober_answers_without_going_out() {
        let adapters = adapters();
        let url = escrow_domain::url::normalize_item("https://x.com/i/status/20")
            .unwrap()
            .0;

        let observed = adapters
            .probe(&url, ContentType::XPost, &Anytime)
            .await
            .unwrap();

        assert_eq!(observed, Presence::Unknown);
    }

    /// 全種別に取得の担当がいること。種別を足したらここで気づく。
    #[test]
    fn every_type_has_someone_to_fetch_it() {
        let adapters = adapters();
        let admit = Anytime;
        for content_type in ContentType::ALL {
            let _ = adapters.acquirer(content_type, &admit);
        }
    }

    #[test]
    fn a_type_belongs_to_exactly_one_platform() {
        for content_type in ContentType::ALL {
            let expected = if content_type.as_str().starts_with("youtube") {
                Platform::Youtube
            } else {
                Platform::X
            };
            assert_eq!(content_type.platform(), expected, "{content_type}");
        }
    }
}

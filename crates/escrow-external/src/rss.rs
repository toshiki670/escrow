//! YouTube の検知（#5）。
//!
//! `https://www.youtube.com/feeds/videos.xml?channel_id=<id>`
//!
//! 外部ツールではないが、外へ出る点は同じなのでこの crate に置く（#7 Phase 3）。
//!
//! 認証は要らず、直近の数件を返す。実測は #5 に在る。
//!
//! # 1件から分かること
//!
//! `link` と `title` と `published` だけ。**配信かどうかの印も、開始時刻も無い。**
//! `/shorts/` と `/watch?v=` は `link` で分かるが、`/watch?v=` の側は動画か配信かを
//! 語らない。そこだけ1件ごとの追加取得で埋める（[`crate::ytdlp::YtDlp::schedule`]）。

use std::time::Duration;

use serde::Deserialize;

use crate::AdapterError;
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::{self, NormalizedUrl, TypeHint};

const PROGRAM: &str = "youtube-rss";

/// 配信元の URL から、その配信元のフィードの場所。
///
/// `Source.url` は `https://www.youtube.com/channel/<UC...>` に正規化済みなので
/// （#1）、末尾の不変 ID をそのまま渡せる。
pub fn feed_url(source: &NormalizedUrl) -> Option<String> {
    let channel_id = source
        .as_str()
        .strip_prefix("https://www.youtube.com/channel/")?;

    if channel_id.is_empty() || channel_id.contains('/') {
        return None;
    }
    Some(format!(
        "https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}"
    ))
}

// ---------------------------------------------------------- 出力の読み取り

/// フィードの、escrow が使う要素だけ。
///
/// 知らない要素は無視する。`<feed>` 直下にも `<link>` が並ぶが、ここで受けるのは
/// `<entry>` だけなので混ざらない。
///
/// **`channelId` を必須にしている。** 項目が0件なのは、まだ何も上げていない
/// 配信元でありうる正しい姿だが、エラーページを渡された姿でもある。区別が付かないと
/// 「消えたチャンネル」が「新着なし」として静かに通る。フィードなら必ず在って
/// エラーページには無いものを1つ要求して、そこで分ける。
#[derive(Debug, Deserialize)]
struct Feed {
    #[serde(rename = "channelId")]
    _channel_id: String,
    #[serde(default)]
    entry: Vec<FeedEntry>,
}

#[derive(Debug, Deserialize)]
struct FeedEntry {
    title: String,
    link: EntryLink,
    /// 枠を作った時刻。予約枠は、作られた瞬間にここへ出る（#5）。
    published: String,
}

#[derive(Debug, Deserialize)]
struct EntryLink {
    #[serde(rename = "@href")]
    href: String,
}

/// フィードで見つけた1件。
///
/// [`crate::Found`] にはまだ足りない — `/watch?v=` の側は種別が
/// 決まっていない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub url: NormalizedUrl,
    /// `link` が語る種別。`/shorts/` なら決まり、`/watch?v=` なら決まらない。
    ///
    /// **正規化する前の `link` から決める**（#1）。`/shorts/<id>` を
    /// `/watch?v=<id>` へ潰すと、ショートと動画を分ける唯一の手掛かりが消える。
    pub hint: TypeHint,
    pub title: String,
    pub published_at: Timestamp,
}

pub fn parse_feed(xml: &str) -> Result<Vec<Sighting>, AdapterError> {
    let feed: Feed = quick_xml::de::from_str(xml).map_err(|e| parse_error(&e))?;

    feed.entry
        .into_iter()
        .map(|entry| {
            let (url, hint) = url::normalize_item(&entry.link.href).map_err(|e| parse_error(&e))?;

            Ok(Sighting {
                url,
                hint,
                title: entry.title,
                published_at: Timestamp::parse(&entry.published).map_err(|e| parse_error(&e))?,
            })
        })
        .collect()
}

fn parse_error(detail: &dyn std::fmt::Display) -> AdapterError {
    AdapterError::Parse {
        program: PROGRAM.to_owned(),
        detail: detail.to_string(),
    }
}

// ------------------------------------------------------------------ 実行

/// フィードを取ってくるもの。
///
/// cookie を持たない。#5 の「YouTube の検知は認証不要」を、渡す手段を持たない形で
/// 守る。繰り返し叩く経路が匿名なので、**賭けているものがアカウントではなくなる**。
#[derive(Debug, Clone)]
pub struct Rss {
    client: reqwest::Client,
}

impl Default for Rss {
    fn default() -> Self {
        Self::new()
    }
}

impl Rss {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// 配信元のフィードを読む。
    pub async fn sightings(&self, source: &NormalizedUrl) -> Result<Vec<Sighting>, AdapterError> {
        let feed = feed_url(source).ok_or_else(|| {
            parse_error(&format!(
                "YouTube の配信元として読めない: {}",
                source.as_str()
            ))
        })?;

        let response = self
            .client
            .get(&feed)
            .send()
            .await
            .map_err(|e| transient(&e))?;

        // 消えたチャンネルは 404。断定できるのはこれだけで、他は判定保留（#5）。
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AdapterError::Unavailable {
                url: source.as_str().to_owned(),
            });
        }
        // 断られたことは一時的な失敗と分ける（#13）。**この経路にだけ合図が在る** —
        // yt-dlp と gallery-dl はどの失敗でも終了コードが 1 で、文言から拒否を
        // 読み取るのは当てにならない（#5）。
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AdapterError::Rejected {
                program: PROGRAM.to_owned(),
                detail: response.status().to_string(),
                retry_after: retry_after(response.headers()),
            });
        }
        let response = response.error_for_status().map_err(|e| transient(&e))?;
        let xml = response.text().await.map_err(|e| transient(&e))?;

        parse_feed(&xml)
    }
}

/// `Retry-After` が指す待ち時間。
///
/// 読むのは秒数の形だけ。HTTP-date の形は空を返し、#2 の既定へ落とす — 日時の書式を
/// 自前で解くより、設定した待ち時間を使うほうが外れ方が小さい。
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_secs)
}

fn transient(detail: &dyn std::fmt::Display) -> AdapterError {
    AdapterError::Transient {
        program: PROGRAM.to_owned(),
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_domain::content::ContentType;

    /// 実物を固めたもの。CNN のチャンネル、2026-09-03 取得。
    const FEED: &str = include_str!("../tests/fixtures/rss/videos.xml");

    fn source(url: &str) -> NormalizedUrl {
        url::normalize_source(url).expect(url)
    }

    #[test]
    fn the_feed_lives_under_the_channels_immutable_id() {
        assert_eq!(
            feed_url(&source(
                "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ"
            )),
            Some(
                "https://www.youtube.com/feeds/videos.xml?channel_id=UCBR8-60-B28hp2BmDPdntcQ"
                    .to_owned()
            )
        );
    }

    /// 秒数の `Retry-After` は読み、それ以外は空にして既定へ落とす。
    #[test]
    fn retry_after_is_read_only_in_its_seconds_form() {
        let header = |value: &str| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::RETRY_AFTER,
                reqwest::header::HeaderValue::from_str(value).unwrap(),
            );
            retry_after(&headers)
        };

        assert_eq!(header("120"), Some(Duration::from_secs(120)));
        assert_eq!(header(" 120 "), Some(Duration::from_secs(120)));
        assert_eq!(header("0"), Some(Duration::ZERO));

        // HTTP-date の形。読まずに空を返す。
        assert_eq!(header("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(header("しばらく"), None);
        assert_eq!(header("-5"), None);

        assert_eq!(retry_after(&reqwest::header::HeaderMap::new()), None);
    }

    /// X の配信元にはフィードが無い。
    #[test]
    fn other_platforms_have_no_feed() {
        assert_eq!(feed_url(&source("https://x.com/i/user/12")), None);
    }

    /// #5 の実測「直近15件」。
    #[test]
    fn the_feed_carries_fifteen_entries() {
        assert_eq!(parse_feed(FEED).unwrap().len(), 15);
    }

    /// ショートは `link` で決まる。#1 の「種別は正規化する前の入口から決める」。
    #[test]
    fn shorts_are_told_apart_by_their_link() {
        let sightings = parse_feed(FEED).unwrap();

        let shorts = sightings
            .iter()
            .filter(|s| s.hint == TypeHint::Known(ContentType::YoutubeShorts))
            .count();
        assert_eq!(shorts, 11);

        // 正規形はどちらも /watch?v= に潰れるので、URL からは区別できない。
        let short = sightings
            .iter()
            .find(|s| s.hint == TypeHint::Known(ContentType::YoutubeShorts))
            .unwrap();
        assert!(
            short
                .url
                .as_str()
                .starts_with("https://www.youtube.com/watch?v=")
        );
    }

    /// **フィードは配信かどうかを語らない。**
    ///
    /// `wwJV2mo2US4` は取得した時点で配信中だったが、他の `/watch?v=` の項目と
    /// 見分けが付かない。動画か配信かも、開始時刻も、ここには無い。だから1件ごとの
    /// 追加取得が要る（#5）。
    #[test]
    fn a_live_entry_looks_exactly_like_a_video() {
        let sightings = parse_feed(FEED).unwrap();

        let undecided: Vec<_> = sightings
            .iter()
            .filter(|s| s.hint == TypeHint::YoutubeUnknown)
            .collect();
        assert_eq!(undecided.len(), 4);

        let live = undecided
            .iter()
            .find(|s| s.url.as_str().ends_with("wwJV2mo2US4"))
            .expect("配信中だった項目が居ること");
        assert_eq!(live.hint, TypeHint::YoutubeUnknown);
    }

    /// 題と日時が読めること。
    #[test]
    fn an_entry_carries_its_title_and_the_time_its_slot_was_made() {
        let sightings = parse_feed(FEED).unwrap();
        let live = sightings
            .iter()
            .find(|s| s.url.as_str().ends_with("wwJV2mo2US4"))
            .unwrap();

        assert!(!live.title.is_empty());
        assert_eq!(
            live.published_at,
            Timestamp::parse("2026-09-03T11:16:56+00:00").unwrap()
        );
    }

    /// フィードでないものを渡されたら落ちること。
    ///
    /// 黙って空の一覧を返すと、エラーページが「新着なし」として通ってしまう。
    #[test]
    fn something_that_is_not_a_feed_is_a_parse_error() {
        for not_a_feed in [
            "<html>お探しのページは見つかりません</html>",
            "まったく XML ではない",
            // 形は合っているが、フィードなら必ず在るものが無い。
            "<feed><entry><title>t</title></entry></feed>",
        ] {
            assert!(
                matches!(parse_feed(not_a_feed), Err(AdapterError::Parse { .. })),
                "{not_a_feed}"
            );
        }
    }

    /// まだ何も上げていない配信元は、空の一覧として通る。
    #[test]
    fn a_feed_with_no_entries_is_not_an_error() {
        let empty = r#"<feed xmlns:yt="http://www.youtube.com/xml/schemas/2015">
             <yt:channelId>UCBR8-60-B28hp2BmDPdntcQ</yt:channelId>
           </feed>"#;

        assert_eq!(parse_feed(empty).unwrap(), Vec::new());
    }
}

//! yt-dlp のアダプタ。
//!
//! #5 の対応表では YouTube の検知と取得、X の Space とライブ配信の取得を持つ。
//!
//! # 検知が2段になる理由
//!
//! `--flat-playlist` は URL と題は返すが **`timestamp` を返さない**（実測。全件 `None`）。
//! #1 は `published_at` を必須にし、監視対象を `Source.created_at` 以降と決めているので、
//! 一覧だけでは `Item` を作れない。
//!
//! そこで口を2つに割る。
//!
//! 1. [`YtDlp::list`] — 並んでいる項目の URL を挙げる。安い
//! 2. [`YtDlp::describe`] — 1件の中身を取る。1リクエスト
//!
//! 間に「台帳に在るか」の判定が入る。これは配信元ではなく escrow が持つ知識なので、
//! アダプタには持たせない。どこまで遡るかの方針も呼ぶ側（Phase 4）が決める。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::invocation::{Completed, Invocation, run};
use super::{Acquire, AdapterError, Discover, Found, Probe};
use crate::asset::{self, Asset, AssetKind};
use crate::config::Browser;
use crate::content::{Content, ContentType, MediaType};
use crate::liveness::Presence;
use crate::source::Source;
use crate::state::MediaPresence;
use crate::timestamp::Timestamp;
use crate::url::{self, NormalizedUrl};

const PROGRAM: &str = "yt-dlp";

/// このアダプタが cookie を取り出せるブラウザ。
///
/// `--cookies-from-browser` が挙げるもの。escrow の [`Browser`] がこの部分集合で
/// あることは `adapter` のテストが確かめる。yt-dlp は `whale` も受けるが、
/// 他のアダプタが受けないので [`Browser`] には入っていない。
pub const SUPPORTED_BROWSERS: &[Browser] = &[
    Browser::Brave,
    Browser::Chrome,
    Browser::Chromium,
    Browser::Edge,
    Browser::Firefox,
    Browser::Opera,
    Browser::Safari,
    Browser::Vivaldi,
];

pub struct YtDlp {
    program: PathBuf,
}

impl YtDlp {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

/// 配信元に並んでいる項目1件。まだ中身は取っていない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub url: NormalizedUrl,
    /// どのタブから見つけたかで決まる（#1 の「種別は正規化する前の入口から決める」）。
    pub content_type: ContentType,
}

/// チャンネルのどのタブを見るか。#5 の `/videos` `/streams` `/shorts`。
///
/// タブと種別が1対1なので、ここで種別が決まる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Videos,
    Streams,
    Shorts,
}

impl Tab {
    pub const ALL: [Self; 3] = [Self::Videos, Self::Streams, Self::Shorts];

    const fn path(self) -> &'static str {
        match self {
            Self::Videos => "videos",
            Self::Streams => "streams",
            Self::Shorts => "shorts",
        }
    }

    const fn content_type(self) -> ContentType {
        match self {
            Self::Videos => ContentType::YoutubeVideo,
            Self::Streams => ContentType::YoutubeLive,
            Self::Shorts => ContentType::YoutubeShorts,
        }
    }

    const fn media_type(self) -> MediaType {
        match self {
            Self::Videos => MediaType::YoutubeVideo,
            Self::Streams => MediaType::YoutubeLive,
            Self::Shorts => MediaType::YoutubeShorts,
        }
    }
}

// ------------------------------------------------------------ 引数の組み立て
//
// すべて純関数。テストはプロセスを起動せず argv を突き合わせるだけで済むので、
// ツールのフラグが変わったときに落ちるのはこの層だけになる。

/// タブに並んでいるものを挙げる。
pub fn list_argv(program: &Path, source_url: &NormalizedUrl, tab: Tab) -> Invocation {
    Invocation::new(program)
        .arg("--ignore-config")
        .arg("--no-warnings")
        .arg("--flat-playlist")
        .arg("--dump-json")
        .arg(format!("{}/{}", source_url.as_str(), tab.path()))
}

/// 1件の中身を取る。
pub fn describe_argv(program: &Path, url: &NormalizedUrl) -> Invocation {
    Invocation::new(program)
        .arg("--ignore-config")
        .arg("--no-warnings")
        .arg("--skip-download")
        .arg("--dump-json")
        .arg(url.as_str())
}

/// 配信元にまだ在るかを確かめる。
pub fn probe_argv(program: &Path, url: &NormalizedUrl) -> Invocation {
    Invocation::new(program)
        .arg("--ignore-config")
        .arg("--no-warnings")
        .arg("--simulate")
        .arg("--quiet")
        .args(["--print", "%(availability)s"])
        .arg(url.as_str())
}

/// 実体を落とす。
///
/// 出力の名前は #1 の `<kind>.<ordinal>.<ext>`。拡張子は yt-dlp が決めるので、
/// こちらは stem だけ指定して、落ちたものを後から走査する。
///
/// 配信は `--live-from-start` で頭から録る。予約枠を待つことはしない（#5）。
pub fn download_argv(program: &Path, url: &NormalizedUrl, into: &Path) -> Invocation {
    Invocation::new(program)
        .arg("--ignore-config")
        .arg("--no-warnings")
        .arg("--no-playlist")
        .arg("--live-from-start")
        .arg("--paths")
        .arg(into)
        .arg("--output")
        .arg(format!("{}.1.%(ext)s", AssetKind::Video.as_str()))
        .arg(url.as_str())
}

// ---------------------------------------------------------- 出力の読み取り
//
// こちらも純関数。fixture で offline にテストできるので、ツールの出力形式が
// 変わったときに落ちるのはこの層だけになる。

/// `--flat-playlist --dump-json` は1行に1件を書く。
#[derive(Debug, Deserialize)]
struct FlatEntry {
    url: String,
}

/// `--dump-json` の、escrow が使う項目だけ。
///
/// 知らないキーは無視する。yt-dlp は項目を足すので、`deny_unknown_fields` に
/// すると足された日に読めなくなる。
#[derive(Debug, Deserialize)]
struct VideoMetadata {
    webpage_url: String,
    title: String,
    /// 投稿日時。Unix 秒。
    timestamp: Option<i64>,
    /// 配信の予定時刻。予約枠にはこちらしか無いことがある。
    release_timestamp: Option<i64>,
}

pub fn parse_list(stdout: &str, tab: Tab) -> Result<Vec<Listed>, AdapterError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let entry: FlatEntry = serde_json::from_str(line).map_err(|e| parse_error(&e))?;
            let (url, _) = url::normalize_item(&entry.url).map_err(|e| parse_error(&e))?;
            Ok(Listed {
                url,
                content_type: tab.content_type(),
            })
        })
        .collect()
}

pub fn parse_describe(stdout: &str, media_type: MediaType) -> Result<Found, AdapterError> {
    let meta: VideoMetadata = serde_json::from_str(stdout.trim()).map_err(|e| parse_error(&e))?;

    let (url, _) = url::normalize_item(&meta.webpage_url).map_err(|e| parse_error(&e))?;
    let seconds = meta
        .timestamp
        .or(meta.release_timestamp)
        .ok_or_else(|| parse_error(&"timestamp も release_timestamp も無い"))?;
    let published_at = epoch_seconds(seconds)?;

    Ok(Found {
        url,
        published_at,
        content: Content::Media {
            media_type,
            title: meta.title,
        },
        // yt-dlp が扱うものは、どれも落とす実体を持つ。
        media: MediaPresence::Present,
    })
}

/// 生存確認の読み取り。
///
/// #5 の非対称性をそのまま写す。**「在る」と読めたときだけ** [`Presence::Present`]。
pub fn parse_probe(completed: &Completed) -> Presence {
    if completed.success && !completed.stdout.trim().is_empty() {
        return Presence::Present;
    }
    if says_gone(&completed.stderr) {
        return Presence::Gone;
    }
    // 終了コードは失敗すると一律 1 なので、読めなかったものは判定保留（#5）。
    Presence::Unknown
}

/// 消えたと断定できる応答か。
///
/// **一覧の保守が正しさを左右しない。** 外れれば `Unknown` になり、`holding` の
/// まま次の回へ回るだけ。当たれば `kept` へ早く移せる、という便宜（#5）。
fn says_gone(stderr: &str) -> bool {
    const GONE: [&str; 4] = [
        "Video unavailable",
        "This video is unavailable",
        "This video has been removed",
        "does not exist",
    ];
    GONE.iter().any(|marker| stderr.contains(marker))
}

fn says_auth_expired(stderr: &str) -> bool {
    const AUTH: [&str; 3] = [
        "Sign in to confirm",
        "cookies are no longer valid",
        "This video is private",
    ];
    AUTH.iter().any(|marker| stderr.contains(marker))
}

fn epoch_seconds(seconds: i64) -> Result<Timestamp, AdapterError> {
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|utc| Timestamp::from(utc.fixed_offset()))
        .ok_or_else(|| parse_error(&format!("日時として読めない unix 秒 {seconds}")))
}

fn parse_error(detail: &dyn std::fmt::Display) -> AdapterError {
    AdapterError::Parse {
        program: PROGRAM.to_owned(),
        detail: detail.to_string(),
    }
}

/// 失敗した呼び出しを、壊れ方で分ける。
fn classify(completed: &Completed, url: &NormalizedUrl) -> AdapterError {
    if says_auth_expired(&completed.stderr) {
        AdapterError::AuthExpired
    } else if says_gone(&completed.stderr) {
        AdapterError::Unavailable {
            url: url.as_str().to_owned(),
        }
    } else {
        AdapterError::Transient {
            program: PROGRAM.to_owned(),
            detail: completed.stderr_tail(),
        }
    }
}

// ------------------------------------------------------------------ 実行

impl YtDlp {
    /// タブに並んでいるものを、配信元が返す順で挙げる。
    ///
    /// YouTube のタブは新しい順に並ぶ。どこまで遡るかは呼ぶ側が決める — 一覧には
    /// 日時が載らないので、遡る判断には [`YtDlp::describe`] が要る。
    pub async fn list(
        &self,
        source_url: &NormalizedUrl,
        tab: Tab,
    ) -> Result<Vec<Listed>, AdapterError> {
        let invocation = list_argv(&self.program, source_url, tab);
        let completed = run(&invocation, None).await?;

        if !completed.success {
            return Err(classify(&completed, source_url));
        }
        parse_list(&completed.stdout, tab)
    }

    /// 1件の中身を取る。
    pub async fn describe(
        &self,
        url: &NormalizedUrl,
        media_type: MediaType,
    ) -> Result<Found, AdapterError> {
        let invocation = describe_argv(&self.program, url);
        let completed = run(&invocation, None).await?;

        if !completed.success {
            return Err(classify(&completed, url));
        }
        parse_describe(&completed.stdout, media_type)
    }
}

impl Discover for YtDlp {
    /// 全タブを見て、`since` 以降のものを返す。
    ///
    /// 台帳との突き合わせをしないので、既に在るものも返る。取り除くのは呼ぶ側。
    async fn discover(
        &self,
        source: &Source,
        since: Timestamp,
    ) -> Result<Vec<Found>, AdapterError> {
        let mut found = Vec::new();

        for tab in Tab::ALL {
            for listed in self.list(&source.url, tab).await? {
                let described = self.describe(&listed.url, tab.media_type()).await?;
                // 配信元が新しい順に並べるので、古いものが出たらこのタブは終わり。
                if described.published_at < since {
                    break;
                }
                found.push(described);
            }
        }

        Ok(found)
    }
}

impl Acquire for YtDlp {
    async fn acquire(
        &self,
        url: &NormalizedUrl,
        _content_type: ContentType,
        into: &Path,
    ) -> Result<Vec<Asset>, AdapterError> {
        std::fs::create_dir_all(into).map_err(|source| AdapterError::Launch {
            program: PROGRAM.to_owned(),
            source,
        })?;

        let invocation = download_argv(&self.program, url, into);
        let completed = run(&invocation, None).await?;

        if !completed.success {
            return Err(classify(&completed, url));
        }

        // 何が落ちたかは、決め打ちせずディレクトリから読む。拡張子は yt-dlp が決める。
        let assets = asset::scan_dir(into).map_err(|source| AdapterError::Launch {
            program: PROGRAM.to_owned(),
            source,
        })?;

        if assets.is_empty() {
            return Err(AdapterError::Parse {
                program: PROGRAM.to_owned(),
                detail: "成功したが実体が置かれていない".to_owned(),
            });
        }
        Ok(assets)
    }
}

impl Probe for YtDlp {
    async fn probe(&self, url: &NormalizedUrl) -> Result<Presence, AdapterError> {
        let invocation = probe_argv(&self.program, url);
        let completed = run(&invocation, None).await?;

        Ok(parse_probe(&completed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program() -> PathBuf {
        PathBuf::from("/opt/homebrew/bin/yt-dlp")
    }

    fn channel() -> NormalizedUrl {
        url::normalize_source("https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ").unwrap()
    }

    fn video() -> NormalizedUrl {
        url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
            .unwrap()
            .0
    }

    // ---- 引数の組み立て。プロセスは起動しない ----

    #[test]
    fn the_listing_call_asks_for_a_flat_json_listing() {
        let invocation = list_argv(&program(), &channel(), Tab::Streams);

        assert_eq!(invocation.program_name(), "yt-dlp");
        assert_eq!(
            invocation.args_as_str().unwrap(),
            [
                "--ignore-config",
                "--no-warnings",
                "--flat-playlist",
                "--dump-json",
                "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ/streams",
            ]
        );
    }

    /// 利用者の設定ファイルに引きずられないこと。手元の `~/.config/yt-dlp` が
    /// 出力形式を変えていると、読み取りの層が理由なく落ちる。
    #[test]
    fn every_call_ignores_the_users_own_config() {
        for invocation in [
            list_argv(&program(), &channel(), Tab::Videos),
            describe_argv(&program(), &video()),
            probe_argv(&program(), &video()),
            download_argv(&program(), &video(), Path::new("/tmp/42")),
        ] {
            assert!(
                invocation
                    .args_as_str()
                    .unwrap()
                    .contains(&"--ignore-config"),
                "{invocation:?}"
            );
        }
    }

    #[test]
    fn the_download_call_records_from_the_start_and_names_the_output() {
        let invocation = download_argv(&program(), &video(), Path::new("/Movies/escrow/42"));
        let args = invocation.args_as_str().unwrap();

        // #5「配信は --live-from-start で頭から録る。予約枠は待たない」
        assert!(args.contains(&"--live-from-start"));
        assert!(!args.iter().any(|a| a.starts_with("--wait-for-video")));
        // #1 の `<kind>.<ordinal>.<ext>`。拡張子は yt-dlp が決める。
        assert!(args.contains(&"video.1.%(ext)s"));
        assert!(args.contains(&"/Movies/escrow/42"));
    }

    // ---- 出力の読み取り。実物の fixture で ----

    const FLAT_PLAYLIST: &str = include_str!("../../tests/fixtures/ytdlp/flat-playlist.jsonl");

    #[test]
    fn reads_a_real_flat_listing() {
        let listed = parse_list(FLAT_PLAYLIST, Tab::Videos).unwrap();

        assert_eq!(listed.len(), 3);
        for entry in &listed {
            assert_eq!(entry.content_type, ContentType::YoutubeVideo);
            assert!(
                entry
                    .url
                    .as_str()
                    .starts_with("https://www.youtube.com/watch?v=")
            );
        }
    }

    /// タブが種別を決める。正規化した URL からは分からない（#1）。
    #[test]
    fn the_tab_decides_the_type() {
        for (tab, expected) in [
            (Tab::Videos, ContentType::YoutubeVideo),
            (Tab::Streams, ContentType::YoutubeLive),
            (Tab::Shorts, ContentType::YoutubeShorts),
        ] {
            let listed = parse_list(FLAT_PLAYLIST, tab).unwrap();
            assert_eq!(listed[0].content_type, expected);
        }
    }

    #[test]
    fn reads_the_metadata_of_one_item() {
        let json = r#"{"webpage_url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                       "title":"Never Gonna Give You Up","timestamp":1256453853,
                       "duration":213,"live_status":"not_live"}"#;

        let found = parse_describe(json, MediaType::YoutubeVideo).unwrap();

        assert_eq!(
            found.url.as_str(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(found.published_at.to_text(), "2009-10-25T06:57:33+00:00");
        assert_eq!(found.content_type(), ContentType::YoutubeVideo);
        assert_eq!(
            found.content,
            Content::Media {
                media_type: MediaType::YoutubeVideo,
                title: "Never Gonna Give You Up".to_owned(),
            }
        );
    }

    /// yt-dlp は出力の項目を足す。知らないキーで落ちてはいけない。
    #[test]
    fn unknown_keys_do_not_break_the_reader() {
        let json = r#"{"webpage_url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                       "title":"t","timestamp":1256453853,
                       "some_new_field_yt_dlp_added":{"nested":true}}"#;

        assert!(parse_describe(json, MediaType::YoutubeVideo).is_ok());
    }

    /// 予約枠には `timestamp` が無く `release_timestamp` だけのことがある。
    #[test]
    fn falls_back_to_the_scheduled_time() {
        let json = r#"{"webpage_url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                       "title":"t","timestamp":null,"release_timestamp":1256453853}"#;

        let found = parse_describe(json, MediaType::YoutubeLive).unwrap();
        assert_eq!(found.published_at.to_text(), "2009-10-25T06:57:33+00:00");
    }

    /// 出力形式が変わったら、**読み取りの層だけ**が落ちる。
    #[test]
    fn a_changed_output_shape_is_a_parse_error() {
        for broken in [
            "not json at all",
            r#"{"title":"t","timestamp":1}"#, // webpage_url が無い
            r#"{"webpage_url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ","title":"t"}"#, // 日時が無い
        ] {
            assert!(
                matches!(
                    parse_describe(broken, MediaType::YoutubeVideo),
                    Err(AdapterError::Parse { .. })
                ),
                "{broken}"
            );
        }
    }

    // ---- 壊れ方の切り分け ----

    #[test]
    fn presence_follows_the_asymmetry() {
        let present = Completed {
            success: true,
            stdout: "public\n".to_owned(),
            stderr: String::new(),
        };
        assert_eq!(parse_probe(&present), Presence::Present);

        let gone = Completed {
            success: false,
            stdout: String::new(),
            stderr: "ERROR: [youtube] aaa: Video unavailable".to_owned(),
        };
        assert_eq!(parse_probe(&gone), Presence::Gone);

        // 読めなかったものは、どれも判定保留。沈黙で捨てない（#5）。
        for stderr in [
            "ERROR: Unable to download webpage: Failed to resolve",
            "ERROR: なにか知らない文言",
            "",
        ] {
            let unknown = Completed {
                success: false,
                stdout: String::new(),
                stderr: stderr.to_owned(),
            };
            assert_eq!(parse_probe(&unknown), Presence::Unknown, "{stderr}");
        }
    }

    /// 成功していても出力が空なら「在る」とは言わない。
    #[test]
    fn an_empty_answer_is_not_a_confirmation() {
        let empty = Completed {
            success: true,
            stdout: "  \n".to_owned(),
            stderr: String::new(),
        };
        assert_eq!(parse_probe(&empty), Presence::Unknown);
    }

    /// cookie の失効はプラットフォーム全体の問題。項目の `error` にしない（#5）。
    #[test]
    fn expired_cookies_are_their_own_failure() {
        let completed = Completed {
            success: false,
            stdout: String::new(),
            stderr: "ERROR: Sign in to confirm you're not a bot".to_owned(),
        };

        let error = classify(&completed, &video());
        assert!(matches!(error, AdapterError::AuthExpired));
        // 生存確認から見れば判定保留。消えたわけではない。
        assert_eq!(error.presence(), Presence::Unknown);
    }

    #[test]
    fn a_gone_item_is_not_a_transient_failure() {
        let completed = Completed {
            success: false,
            stdout: String::new(),
            stderr: "ERROR: [youtube] x: Video unavailable".to_owned(),
        };

        let error = classify(&completed, &video());
        assert!(matches!(error, AdapterError::Unavailable { .. }));
        assert_eq!(error.presence(), Presence::Gone);
    }
}

//! yt-dlp のアダプタ。
//!
//! #5 の対応表では YouTube の取得、X の Space とライブ配信の取得、それに
//! YouTube の検知の**追加取得**を持つ。配信元に並んでいるものを挙げるのは
//! [`crate::rss`] で、こちらはやらない。
//!
//! # 認証は経路ごとに決まる
//!
//! #5 が「認証は人が明示した対象にしか掛からない」と決めたので、cookie は共通の
//! 前置きから外し、要る呼び出しだけが足す。
//!
//! | 呼び出し | 認証 | 理由 |
//! |---|---|---|
//! | [`schedule_argv`] | 無し | 検知の追加取得。繰り返し叩くので匿名に保つ |
//! | [`describe_argv`] | cookie | 人が登録した URL。メン限がありうる |
//! | [`probe_argv`] | cookie | 生存確認。匿名で足りるかは未検証（#5） |
//! | [`download_argv`] | cookie | メン限の取得に要る |

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::invocation::{Completed, Invocation, run};
use crate::{Acquire, AdapterError, Found, Probe};
use escrow_config::Browser;
use escrow_domain::asset::{self, Asset, AssetKind};
use escrow_domain::content::{Content, MediaType};
use escrow_domain::liveness::Presence;
use escrow_domain::state::MediaPresence;
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::{self, NormalizedUrl};

const PROGRAM: &str = "yt-dlp";

/// このアダプタが cookie を取り出せるブラウザ。
///
/// `--cookies-from-browser` が挙げるもの。escrow の [`Browser`] がこの部分集合で
/// あることは `escrow-external` の `every_configurable_browser_works_with_every_adapter` が確かめる。yt-dlp は `whale` も受けるが、
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
    browser: Browser,
}

impl YtDlp {
    pub fn new(program: impl Into<PathBuf>, browser: Browser) -> Self {
        Self {
            program: program.into(),
            browser,
        }
    }
}

/// 追加取得で分かること。フィードに無いのはこの2つだけ（#5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    /// 動画か配信か。`live_status` が決める。
    pub media_type: MediaType,
    /// 予約枠なら開始予定時刻。始まってしまった配信には入らない。
    pub scheduled_start_at: Option<Timestamp>,
}

// ------------------------------------------------------------ 引数の組み立て

/// どの呼び出しにも渡すもの。**cookie はここに入れない。**
fn base(program: &Path) -> Invocation {
    Invocation::new(program)
        // 利用者の設定ファイルに引きずられない。出力形式を変えられていると、
        // 読み取りの層が理由なく落ちる。
        .arg("--ignore-config")
        .arg("--no-warnings")
}

/// cookie を足す。取り出し元は #2 の1つの設定から来る。
fn with_cookies(invocation: Invocation, browser: Browser) -> Invocation {
    invocation
        .arg("--cookies-from-browser")
        .arg(browser.as_str())
}

/// 検知の追加取得。フィードが語らない「動画か配信か」と開始時刻を、ここで埋める。
///
/// 引数は [`describe_argv`] と同じで、違うのは cookie の有無だけ。
pub fn schedule_argv(program: &Path, url: &NormalizedUrl) -> Invocation {
    base(program)
        .arg("--skip-download")
        .arg("--dump-json")
        .arg(url.as_str())
}

/// 人が登録した1件の中身を取る。
pub fn describe_argv(program: &Path, url: &NormalizedUrl, browser: Browser) -> Invocation {
    with_cookies(base(program), browser)
        .arg("--skip-download")
        .arg("--dump-json")
        .arg(url.as_str())
}

/// 配信元にまだ在るかを確かめる。
pub fn probe_argv(program: &Path, url: &NormalizedUrl, browser: Browser) -> Invocation {
    with_cookies(base(program), browser)
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
pub fn download_argv(
    program: &Path,
    url: &NormalizedUrl,
    into: &Path,
    browser: Browser,
) -> Invocation {
    with_cookies(base(program), browser)
        .arg("--no-playlist")
        .arg("--live-from-start")
        .arg("--paths")
        .arg(into)
        .arg("--output")
        .arg(format!("{}.1.%(ext)s", AssetKind::Video.as_str()))
        .arg(url.as_str())
}

// ---------------------------------------------------------- 出力の読み取り

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
    /// 配信かどうかと、いまどの段階か。
    live_status: Option<String>,
}

impl VideoMetadata {
    /// `live_status` を #1 の種別へ写す。
    ///
    /// ショートはここでは決まらない — 正規形が動画と同じ `/watch?v=` なので、
    /// yt-dlp から見て両者は区別が付かない。決めるのはフィードの `link`（#1）。
    ///
    /// アーカイブも配信中も `youtube_live`。#1 の「配信中かアーカイブかは種別
    /// ではない」。
    fn media_type(&self) -> Result<MediaType, AdapterError> {
        match self.live_status.as_deref() {
            Some("is_upcoming" | "is_live" | "post_live" | "was_live") => {
                Ok(MediaType::YoutubeLive)
            }
            // 配信でないもの。値が無い場合も含む（古い yt-dlp、抽出できなかった）。
            Some("not_live") | None => Ok(MediaType::YoutubeVideo),
            Some(unknown) => Err(parse_error(&format!("知らない live_status `{unknown}`"))),
        }
    }

    /// 予約枠の開始予定時刻。
    ///
    /// **`is_upcoming` のときだけ。** 始まってしまえば予約枠ではないので、
    /// #1 の `scheduled_start_at` は空になる（NULL の意味は「予約枠ではない」）。
    fn scheduled_start_at(&self) -> Result<Option<Timestamp>, AdapterError> {
        if self.live_status.as_deref() != Some("is_upcoming") {
            return Ok(None);
        }
        self.release_timestamp.map(epoch_seconds).transpose()
    }
}

/// 検知の追加取得の読み取り。フィードに無い2つだけを返す。
pub fn parse_schedule(stdout: &str) -> Result<Schedule, AdapterError> {
    let meta: VideoMetadata = serde_json::from_str(stdout.trim()).map_err(|e| parse_error(&e))?;

    Ok(Schedule {
        media_type: meta.media_type()?,
        scheduled_start_at: meta.scheduled_start_at()?,
    })
}

pub fn parse_describe(stdout: &str, media_type: MediaType) -> Result<Found, AdapterError> {
    let meta: VideoMetadata = serde_json::from_str(stdout.trim()).map_err(|e| parse_error(&e))?;

    let (url, _) = url::normalize_item(&meta.webpage_url).map_err(|e| parse_error(&e))?;
    let seconds = meta
        .timestamp
        .or(meta.release_timestamp)
        .ok_or_else(|| parse_error(&"timestamp も release_timestamp も無い"))?;
    let published_at = epoch_seconds(seconds)?;
    let scheduled_start_at = meta.scheduled_start_at()?;

    Ok(Found {
        url,
        published_at,
        scheduled_start_at,
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

/// cookie を使えないと言っているか。
///
/// 「入っていないブラウザを設定した」も入れる。実測すると、その場合は認証の
/// 要らない公開のものまで exit 1 で落ちるので、一時的な失敗として扱うと
/// 何を直せばよいか分からないまま止まり続ける。
fn says_unauthenticated(stderr: &str) -> bool {
    const MARKERS: [&str; 5] = [
        "Sign in to confirm",
        "cookies are no longer valid",
        "This video is private",
        "could not find",
        "cookies database",
    ];
    MARKERS.iter().any(|marker| stderr.contains(marker))
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
    if says_unauthenticated(&completed.stderr) {
        AdapterError::Unauthenticated {
            detail: completed.stderr_tail(),
        }
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
    /// 検知の追加取得。フィードに無い2つを取る（#5）。
    ///
    /// **cookie を渡さない。** 検知は繰り返し叩く経路なので匿名に保つ。
    pub async fn schedule(&self, url: &NormalizedUrl) -> Result<Schedule, AdapterError> {
        let invocation = schedule_argv(&self.program, url);
        let completed = run(&invocation, None).await?;

        if !completed.success {
            return Err(classify(&completed, url));
        }
        parse_schedule(&completed.stdout)
    }

    /// 人が登録した1件の中身を取る。
    pub async fn describe(
        &self,
        url: &NormalizedUrl,
        media_type: MediaType,
    ) -> Result<Found, AdapterError> {
        let invocation = describe_argv(&self.program, url, self.browser);
        let completed = run(&invocation, None).await?;

        if !completed.success {
            return Err(classify(&completed, url));
        }
        parse_describe(&completed.stdout, media_type)
    }
}

impl Acquire for YtDlp {
    async fn acquire(&self, url: &NormalizedUrl, into: &Path) -> Result<Vec<Asset>, AdapterError> {
        std::fs::create_dir_all(into).map_err(|source| AdapterError::Launch {
            program: PROGRAM.to_owned(),
            source,
        })?;

        let invocation = download_argv(&self.program, url, into, self.browser);
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
        let invocation = probe_argv(&self.program, url, self.browser);
        let completed = run(&invocation, None).await?;

        Ok(parse_probe(&completed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_domain::content::ContentType;

    fn program() -> PathBuf {
        PathBuf::from("/opt/homebrew/bin/yt-dlp")
    }

    const URL: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

    fn video() -> NormalizedUrl {
        url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
            .unwrap()
            .0
    }

    // ---- 引数の組み立て。プロセスは起動しない ----

    #[test]
    fn the_follow_up_call_asks_for_one_items_json() {
        let invocation = schedule_argv(&program(), &video());

        assert_eq!(invocation.program_name(), "yt-dlp");
        assert_eq!(
            invocation.args_as_str().unwrap(),
            [
                "--ignore-config",
                "--no-warnings",
                "--skip-download",
                "--dump-json",
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            ]
        );
    }

    /// **検知は cookie を渡さない**（#5）。
    ///
    /// 繰り返し叩くのはこの経路なので、匿名に保つことで賭けているものが
    /// アカウントではなくなる。引数が1つでも増えたら落ちるよう、無いことを
    /// 直接見る。
    #[test]
    fn discovery_never_sends_cookies() {
        let args = schedule_argv(&program(), &video());
        let args = args.args_as_str().unwrap();

        assert!(
            !args.contains(&"--cookies-from-browser"),
            "検知に cookie が混ざっている: {args:?}"
        );
        assert!(!args.iter().any(|a| a.contains("cookie")), "{args:?}");
    }

    /// 認証が要る経路は、#2 の1つの設定から来たブラウザを渡す。
    ///
    /// gallery-dl だけ認証が効いて yt-dlp が落ちる、という非対称を作らない。
    #[test]
    fn authenticated_routes_carry_the_configured_browser() {
        for invocation in [
            describe_argv(&program(), &video(), Browser::Safari),
            probe_argv(&program(), &video(), Browser::Safari),
            download_argv(&program(), &video(), Path::new("/tmp/42"), Browser::Safari),
        ] {
            assert!(
                invocation
                    .args_as_str()
                    .unwrap()
                    .windows(2)
                    .any(|w| w == ["--cookies-from-browser", "safari"]),
                "{invocation:?}"
            );
        }
    }

    /// 利用者の設定ファイルに引きずられないこと。手元の `~/.config/yt-dlp` が
    /// 出力形式を変えていると、読み取りの層が理由なく落ちる。
    #[test]
    fn every_call_ignores_the_users_own_config() {
        for invocation in [
            schedule_argv(&program(), &video()),
            describe_argv(&program(), &video(), Browser::Firefox),
            probe_argv(&program(), &video(), Browser::Firefox),
            download_argv(&program(), &video(), Path::new("/tmp/42"), Browser::Firefox),
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
        let invocation = download_argv(
            &program(),
            &video(),
            Path::new("/Movies/escrow/42"),
            Browser::Firefox,
        );
        let args = invocation.args_as_str().unwrap();

        // #5「配信は --live-from-start で頭から録る。予約枠は待たない」
        assert!(args.contains(&"--live-from-start"));
        assert!(!args.iter().any(|a| a.starts_with("--wait-for-video")));
        // #1 の `<kind>.<ordinal>.<ext>`。拡張子は yt-dlp が決める。
        assert!(args.contains(&"video.1.%(ext)s"));
        assert!(args.contains(&"/Movies/escrow/42"));
    }

    // ---- 出力の読み取り。実物の fixture で ----

    /// `live_status` が動画と配信を分ける。実測した値をそのまま並べる。
    ///
    /// アーカイブ（`was_live`）も配信中（`is_live`）も `youtube_live`。#1 の
    /// 「配信中かアーカイブかは種別ではない」。
    #[test]
    fn live_status_decides_video_or_stream() {
        for (status, expected) in [
            ("not_live", MediaType::YoutubeVideo),
            ("is_upcoming", MediaType::YoutubeLive),
            ("is_live", MediaType::YoutubeLive),
            ("post_live", MediaType::YoutubeLive),
            ("was_live", MediaType::YoutubeLive),
        ] {
            let json = format!(r#"{{"webpage_url":"{URL}","title":"t","live_status":"{status}"}}"#);
            assert_eq!(
                parse_schedule(&json).unwrap().media_type,
                expected,
                "{status}"
            );
        }
    }

    /// 知らない `live_status` は読み取りの層で落とす。**ツールの仕様が変わった
    /// 疑い**なので、勝手に動画へ寄せない。
    #[test]
    fn an_unknown_live_status_is_a_parse_error() {
        let json = format!(r#"{{"webpage_url":"{URL}","title":"t","live_status":"is_premiere"}}"#);

        assert!(matches!(
            parse_schedule(&json),
            Err(AdapterError::Parse { .. })
        ));
    }

    /// 開始予定時刻が入るのは**予約枠のときだけ**。
    ///
    /// 始まってしまえば予約枠ではないので空になる（#1 の `NULL` の意味）。
    /// 過ぎた時刻を入れると、#13 がその時刻に取得を予約しても意味が無い。
    #[test]
    fn only_an_upcoming_slot_carries_a_start_time() {
        let upcoming = format!(
            r#"{{"webpage_url":"{URL}","title":"t",
                 "live_status":"is_upcoming","release_timestamp":1788455896}}"#
        );
        assert_eq!(
            parse_schedule(&upcoming).unwrap().scheduled_start_at,
            Some(Timestamp::parse("2026-09-03T17:18:16+00:00").unwrap())
        );

        // 配信中。release_timestamp は入っているが、もう予約枠ではない。
        let live = format!(
            r#"{{"webpage_url":"{URL}","title":"t",
                 "live_status":"is_live","release_timestamp":1788455896}}"#
        );
        assert_eq!(parse_schedule(&live).unwrap().scheduled_start_at, None);
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

    /// cookie を使えないのはプラットフォーム全体の問題。項目の `error` にしない（#5）。
    #[test]
    fn unusable_cookies_are_their_own_failure() {
        for stderr in [
            "ERROR: Sign in to confirm you're not a bot",
            "ERROR: could not find opera cookies database in \"/Users/t/Library/…\"",
        ] {
            let completed = Completed {
                success: false,
                stdout: String::new(),
                stderr: stderr.to_owned(),
            };

            let error = classify(&completed, &video());
            assert!(
                matches!(error, AdapterError::Unauthenticated { .. }),
                "{stderr}"
            );
            // 生存確認から見れば判定保留。消えたわけではない。
            assert_eq!(error.presence(), Presence::Unknown);
        }
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

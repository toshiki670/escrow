//! gallery-dl のアダプタ。
//!
//! 何をこのツールが担うかは [`crate::route`] の対応表が決める。
//!
//! # 検知が1段で済む理由
//!
//! yt-dlp と違い、一覧の時点で日時も本文も返る。2段に割る必要がない。
//!
//! # 名前を決めるのはこちら
//!
//! gallery-dl は自分の規則でファイル名を付けるので、いったん別の場所へ落として
//! から #1 の `<kind>.<ordinal>.<ext>` へ移す。命名規則は #1 が決めたもので、
//! ツールに預けない。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::invocation::{Completed, Invocation, run};
use crate::{AdapterError, Found};
use escrow_config::Browser;
use escrow_domain::asset::{Asset, AssetKind};
use escrow_domain::content::Content;
use escrow_domain::source::Source;
use escrow_domain::state::MediaPresence;
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url::{self, NormalizedUrl};

const PROGRAM: &str = "gallery-dl";

/// このアダプタが cookie を取り出せるブラウザ。
///
/// escrow の [`Browser`] がこの部分集合であることは `escrow-external` の `every_configurable_browser_works_with_every_adapter` が確かめる。
/// gallery-dl は `floorp` / `librewolf` / `orion` / `thorium` / `zen` も受けるが、
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

pub struct GalleryDl {
    program: PathBuf,
    browser: Browser,
}

impl GalleryDl {
    pub fn new(program: impl Into<PathBuf>, browser: Browser) -> Self {
        Self {
            program: program.into(),
            browser,
        }
    }
}

// ------------------------------------------------------------ 引数の組み立て

/// 配信元の正規形から、このツールが読めるタイムラインの URL を導く。
///
/// escrow が持つのは不変の同一性（`x.com/i/user/<id>`）だけで、要求の形は
/// アダプタが組み立てる。別のツールへ替えれば、変わるのはこの関数だけ。
pub(crate) fn timeline_url(source: &NormalizedUrl) -> Option<String> {
    let id = source.as_str().strip_prefix("https://x.com/i/user/")?;
    (!id.is_empty()).then(|| format!("https://x.com/id:{id}/timeline"))
}

/// 共通で渡すもの。
fn base(program: &Path, browser: Browser) -> Invocation {
    Invocation::new(program)
        // 利用者の設定ファイルに引きずられない。出力形式を変えられていると、
        // 読み取りの層が理由なく落ちる。
        .arg("--config-ignore")
        .arg("--cookies-from-browser")
        .arg(browser.as_str())
}

/// タイムラインを列挙する。落とさない。
///
/// `text-tweets` はメディアの無い投稿を拾うため。既定では飛ばされる（#5）。
/// `cards=ytdl` は Space やライブ配信のカードを拾うため。
pub(crate) fn timeline_argv(program: &Path, timeline: &str, browser: Browser) -> Invocation {
    base(program, browser)
        .arg("--dump-json")
        .args(["-o", "extractor.twitter.text-tweets=true"])
        .args(["-o", "extractor.twitter.cards=ytdl"])
        // 引用元と RT は、それ自体を項目にしない。繋がりは URL で記録する（#1）。
        .args(["-o", "extractor.twitter.quoted=false"])
        .args(["-o", "extractor.twitter.retweets=false"])
        .arg(timeline)
}

/// 1つの投稿の中身を取る。落とさない。
///
/// タイムラインと同じ envelope が返るので、読み取りは共通。
pub(crate) fn describe_argv(program: &Path, url: &NormalizedUrl, browser: Browser) -> Invocation {
    base(program, browser)
        .arg("--dump-json")
        .args(["-o", "extractor.twitter.text-tweets=true"])
        .arg(url.as_str())
}

/// 1つの投稿の実体を落とす。
///
/// 出力先は一時の置き場。名前を #1 の規則へ移すのは呼んだ側。
pub(crate) fn download_argv(
    program: &Path,
    url: &NormalizedUrl,
    into: &Path,
    browser: Browser,
) -> Invocation {
    base(program, browser)
        .arg("--directory")
        .arg(into)
        // 種類も通し番号もこちらで決めるので、ここでは順番と拡張子だけ受け取る。
        .args(["--filename", "{num}.{extension}"])
        .arg(url.as_str())
}

// ---------------------------------------------------------- 出力の読み取り

/// `--dump-json` は `[種別, ...]` の配列を出す。
///
/// 種別 2 が投稿の見出し（メタデータ）、3 がその投稿に属するファイル、
/// **-1 が失敗**。実物の出力で確かめてある。
///
/// 失敗が標準エラーではなく出力の中に混ざるので、終了コードや stderr だけを
/// 見ていると取り逃がす。
const FAILED: i64 = -1;
const DIRECTORY: i64 = 2;
const URL: i64 = 3;

/// 投稿のメタデータのうち、escrow が使うものだけ。
///
/// 知らないキーは無視する。gallery-dl は項目を足すので、`deny_unknown_fields` に
/// すると足された日に読めなくなる。
#[derive(Debug, Deserialize)]
struct TweetMetadata {
    tweet_id: i64,
    /// `"2006-03-21 20:50:14"`。時差を UTC へ畳んだ形で、オフセットが付かない。
    date: String,
    content: String,
    /// 返信先。返信でなければ 0。
    #[serde(default)]
    reply_id: i64,
    /// 引用元。引用でなければ 0。
    #[serde(default)]
    quoted_id: i64,
}

pub(crate) fn parse_timeline(stdout: &str) -> Result<Vec<Found>, AdapterError> {
    let entries: Vec<serde_json::Value> =
        serde_json::from_str(stdout.trim()).map_err(|e| parse_error(&e))?;

    let mut found: Vec<Found> = Vec::new();

    for entry in entries {
        let Some(items) = entry.as_array() else {
            return Err(parse_error(&"配列でない要素がある"));
        };
        let Some(kind) = items.first().and_then(serde_json::Value::as_i64) else {
            return Err(parse_error(&"要素の先頭が種別でない"));
        };

        match kind {
            FAILED => return Err(failure(items.get(1))),
            DIRECTORY => {
                let meta = items
                    .get(1)
                    .ok_or_else(|| parse_error(&"見出しに中身が無い"))?;
                found.push(post(meta)?);
            }
            // 直前の投稿に属するファイル。1本でもあれば落とす実体がある。
            URL => match found.last_mut() {
                Some(last) => last.media = MediaPresence::Present,
                None => return Err(parse_error(&"見出しの前にファイルが出た")),
            },
            // 知らない種別を黙って捨てない。投稿を載せた新しい形が来たとき、
            // 取りこぼしが「空のタイムライン」に見える。Parse なので判定は
            // 保留に倒れ、預かり中のものは捨てられない（#5）。
            other => return Err(parse_error(&format!("知らない出力の種別 {other}"))),
        }
    }

    Ok(found)
}

fn post(meta: &serde_json::Value) -> Result<Found, AdapterError> {
    let meta: TweetMetadata = serde_json::from_value(meta.clone()).map_err(|e| parse_error(&e))?;

    let url = status_url(meta.tweet_id)?;

    Ok(Found {
        url,
        // X に予約枠は無い。Space もライブ配信も、始まってから見つかる。
        scheduled_start_at: None,
        published_at: naive_utc(&meta.date)?,
        content: Content::Post {
            body: meta.content,
            in_reply_to: link(meta.reply_id)?,
            quoted: link(meta.quoted_id)?,
        },
        // ファイルが続いていれば、読み取りの側で `Present` に上書きされる。
        media: MediaPresence::Absent,
    })
}

/// 繋がりの URL。0 は「無い」を表す（gallery-dl が欠けた値を 0 にするため）。
fn link(id: i64) -> Result<Option<NormalizedUrl>, AdapterError> {
    match id {
        0 => Ok(None),
        id => status_url(id).map(Some),
    }
}

fn status_url(id: i64) -> Result<NormalizedUrl, AdapterError> {
    url::normalize_item(&format!("https://x.com/i/status/{id}"))
        .map(|(url, _)| url)
        .map_err(|e| parse_error(&e))
}

/// `"2006-03-21 20:50:14"` を読む。時差は畳まれていて UTC。
fn naive_utc(text: &str) -> Result<Timestamp, AdapterError> {
    chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S")
        .map(|naive| Timestamp::from(naive.and_utc().fixed_offset()))
        .map_err(|_| parse_error(&format!("日時として読めない `{text}`")))
}

/// 出力に混ざった失敗を読む。
///
/// 認証切れは個別の項目ではなくプラットフォーム全体の問題なので、`error` を
/// 並べず取得を止めて人に知らせる（#5）。
fn failure(detail: Option<&serde_json::Value>) -> AdapterError {
    let error = detail
        .and_then(|d| d.get("error"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let message = detail
        .and_then(|d| d.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    if error == "AuthRequired" || says_unauthenticated(message) {
        AdapterError::Unauthenticated {
            detail: format!("{error}: {message}"),
        }
    } else {
        AdapterError::Transient {
            program: PROGRAM.to_owned(),
            detail: format!("{error}: {message}"),
        }
    }
}

fn parse_error(detail: &dyn std::fmt::Display) -> AdapterError {
    AdapterError::Parse {
        program: PROGRAM.to_owned(),
        detail: detail.to_string(),
    }
}

fn says_unauthenticated(stderr: &str) -> bool {
    const AUTH: [&str; 6] = [
        "Login required",
        "authorization",
        "Unauthorized",
        "requires authentication",
        "could not find",
        "cookies database",
    ];
    let lowered = stderr.to_ascii_lowercase();
    AUTH.iter()
        .any(|m| lowered.contains(&m.to_ascii_lowercase()))
}

fn classify(completed: &Completed) -> AdapterError {
    if says_unauthenticated(&completed.stderr) {
        AdapterError::Unauthenticated {
            detail: completed.stderr_tail(),
        }
    } else {
        AdapterError::Transient {
            program: PROGRAM.to_owned(),
            detail: completed.stderr_tail(),
        }
    }
}

// ------------------------------------------------------------------ 実行

impl GalleryDl {
    /// 1件の中身を取る。人が URL を登録するときの入口（#5）。
    pub(crate) async fn describe(&self, url: &NormalizedUrl) -> Result<Found, AdapterError> {
        let completed = run(&describe_argv(&self.program, url, self.browser), None).await?;
        if !completed.success {
            return Err(classify(&completed));
        }

        parse_timeline(&completed.stdout)?
            .into_iter()
            .next()
            .ok_or_else(|| AdapterError::Unavailable {
                url: url.as_str().to_owned(),
            })
    }

    /// タイムラインを1回読む。順番待ちは [`crate::route::Discoverer`] が掛ける。
    pub(crate) async fn discover(
        &self,
        source: &Source,
        since: Timestamp,
    ) -> Result<Vec<Found>, AdapterError> {
        let timeline = timeline_url(&source.url).ok_or_else(|| AdapterError::Parse {
            program: PROGRAM.to_owned(),
            detail: format!("X の配信元として読めない: {}", source.url),
        })?;

        let completed = run(&timeline_argv(&self.program, &timeline, self.browser), None).await?;
        if !completed.success {
            return Err(classify(&completed));
        }

        let mut found = parse_timeline(&completed.stdout)?;
        found.retain(|f| f.published_at >= since);
        Ok(found)
    }

    /// 実体を落とす。順番待ちは [`crate::route::Acquirer`] が掛ける。
    pub(crate) async fn acquire(
        &self,
        url: &NormalizedUrl,
        into: &Path,
    ) -> Result<Vec<Asset>, AdapterError> {
        let io_error = |source| AdapterError::Launch {
            program: PROGRAM.to_owned(),
            source,
        };

        // gallery-dl は自分の規則で名前を付けるので、いったん別の場所へ受ける。
        let scratch = tempfile::tempdir().map_err(io_error)?;
        std::fs::create_dir_all(into).map_err(io_error)?;

        let completed = run(
            &download_argv(&self.program, url, scratch.path(), self.browser),
            None,
        )
        .await?;
        if !completed.success {
            return Err(classify(&completed));
        }

        rename_into_place(scratch.path(), into)
    }
}

/// 落ちてきたものを #1 の `<kind>.<ordinal>.<ext>` へ移す。
///
/// 種類は拡張子から当て、通し番号は種類ごとに1から振り直す。X 投稿の画像が
/// 最大4枚なら `image.1.jpg` … `image.4.jpg` になる。
fn rename_into_place(from: &Path, into: &Path) -> Result<Vec<Asset>, AdapterError> {
    let io_error = |source| AdapterError::Launch {
        program: PROGRAM.to_owned(),
        source,
    };

    let mut downloaded: Vec<PathBuf> = std::fs::read_dir(from)
        .map_err(io_error)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    // gallery-dl の `{num}` が並び順を持つので、名前で並べれば投稿内の順になる。
    downloaded.sort();

    // 通し番号は種類ごとに1から振り直す。宣言順に依存しないよう、種類を鍵にする。
    let mut counts: std::collections::HashMap<AssetKind, u32> = std::collections::HashMap::new();
    let mut assets = Vec::new();

    for path in downloaded {
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let Some(kind) = AssetKind::of_extension(extension) else {
            continue;
        };

        let slot = counts.entry(kind).or_default();
        *slot += 1;
        let ordinal = std::num::NonZeroU32::new(*slot).expect("1 から数える");

        let asset = Asset::new(kind, ordinal, extension.to_ascii_lowercase());
        std::fs::rename(&path, into.join(asset.file_name())).map_err(io_error)?;
        assets.push(asset);
    }

    if assets.is_empty() {
        return Err(AdapterError::Parse {
            program: PROGRAM.to_owned(),
            detail: "成功したが実体が置かれていない".to_owned(),
        });
    }

    assets.sort();
    Ok(assets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use escrow_domain::content::ContentType;

    fn program() -> PathBuf {
        PathBuf::from("/opt/homebrew/bin/gallery-dl")
    }

    fn source_url() -> NormalizedUrl {
        url::normalize_source("https://x.com/i/user/12").unwrap()
    }

    fn post_url() -> NormalizedUrl {
        url::normalize_item("https://x.com/jack/status/20")
            .unwrap()
            .0
    }

    // ---- 要求 URL の組み立て。ドメインは同一性だけを持つ ----

    /// 正規形から、このツールが読める形を導く。ツールを替えればここだけ変わる。
    #[test]
    fn the_timeline_url_is_derived_from_the_canonical_identity() {
        assert_eq!(
            timeline_url(&source_url()).unwrap(),
            "https://x.com/id:12/timeline"
        );
    }

    #[test]
    fn a_youtube_source_is_not_a_timeline_here() {
        let youtube =
            url::normalize_source("https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ")
                .unwrap();
        assert_eq!(timeline_url(&youtube), None);
    }

    // ---- 引数の組み立て。プロセスは起動しない ----

    #[test]
    fn the_listing_call_asks_for_what_number_5_decided() {
        let invocation =
            timeline_argv(&program(), "https://x.com/id:12/timeline", Browser::Firefox);
        let args = invocation.args_as_str().unwrap();

        // #5「text-tweets でメディアの無い投稿を拾う」「cards=ytdl でカードを拾う」
        assert!(args.contains(&"extractor.twitter.text-tweets=true"));
        assert!(args.contains(&"extractor.twitter.cards=ytdl"));
        // 引用元と RT はそれ自体を項目にしない（#1 の「1投稿 = 1 Item」）。
        assert!(args.contains(&"extractor.twitter.quoted=false"));
        assert!(args.contains(&"extractor.twitter.retweets=false"));
        assert!(args.contains(&"--dump-json"));
    }

    #[test]
    fn every_call_ignores_the_users_own_config_and_names_the_browser() {
        for invocation in [
            timeline_argv(&program(), "https://x.com/id:12/timeline", Browser::Safari),
            describe_argv(&program(), &post_url(), Browser::Safari),
            download_argv(
                &program(),
                &post_url(),
                Path::new("/tmp/x"),
                Browser::Safari,
            ),
        ] {
            let args = invocation.args_as_str().unwrap();
            assert!(args.contains(&"--config-ignore"), "{invocation:?}");
            assert!(
                args.windows(2)
                    .any(|w| w == ["--cookies-from-browser", "safari"]),
                "{invocation:?}"
            );
        }
    }

    // ---- 出力の読み取り ----

    /// 実物の envelope（`[2, meta]` と `[3, url, meta]`）に、gallery-dl 自身の
    /// `_transform_tweet` が出すキーを載せたもの。
    ///
    /// X はタイムラインの列挙に cookie を要るので、この形は認証を通したうえで
    /// もう一度確かめる必要がある（#5 の「取りこぼしや不便が出てから直す」）。
    const TIMELINE: &str = include_str!("../tests/fixtures/gallerydl/timeline.json");

    #[test]
    fn reads_a_timeline() {
        let found = parse_timeline(TIMELINE).unwrap();
        assert_eq!(found.len(), 3);

        // どれも x_post。Space とライブ配信の取得は yt-dlp が持つ（#5）。
        for entry in &found {
            assert_eq!(entry.content_type(), ContentType::XPost);
        }
    }

    /// ハンドル抜きの正規形へ潰れる（#1）。
    #[test]
    fn urls_are_canonical() {
        let found = parse_timeline(TIMELINE).unwrap();
        assert_eq!(found[0].url.as_str(), "https://x.com/i/status/20");
    }

    /// 見出しに続くファイルがあれば「取得する実体がある」。
    /// 無ければ本文だけなので、#1 のとおり `kept` から始まる。
    #[test]
    fn files_after_a_post_mean_there_is_something_to_fetch() {
        let found = parse_timeline(TIMELINE).unwrap();

        assert_eq!(found[0].media, MediaPresence::Absent, "テキストだけの投稿");
        assert_eq!(found[1].media, MediaPresence::Present, "画像2枚の投稿");
        assert_eq!(found[2].media, MediaPresence::Present, "動画の投稿");
    }

    /// 動画だけの投稿は `body` が空の `Post`。「無い」のではなく「空」（#1）。
    #[test]
    fn a_media_only_post_has_an_empty_body() {
        let found = parse_timeline(TIMELINE).unwrap();

        match &found[2].content {
            Content::Post { body, .. } => assert!(body.is_empty()),
            other => panic!("Post のはず: {other:?}"),
        }
    }

    /// 繋がりは URL で記録する。0 は「無い」（#1）。
    #[test]
    fn links_are_recorded_as_canonical_urls() {
        let found = parse_timeline(TIMELINE).unwrap();

        let Content::Post {
            in_reply_to,
            quoted,
            ..
        } = &found[1].content
        else {
            panic!("Post のはず");
        };
        assert_eq!(
            in_reply_to.as_ref().map(NormalizedUrl::as_str),
            Some("https://x.com/i/status/19")
        );
        assert_eq!(
            quoted.as_ref().map(NormalizedUrl::as_str),
            Some("https://x.com/i/status/18")
        );

        let Content::Post { in_reply_to, .. } = &found[0].content else {
            panic!("Post のはず");
        };
        assert_eq!(*in_reply_to, None, "0 は繋がりが無いこと");
    }

    #[test]
    fn dates_are_read_as_utc() {
        let found = parse_timeline(TIMELINE).unwrap();
        assert_eq!(found[0].published_at.to_text(), "2006-03-21T20:50:14+00:00");
    }

    /// gallery-dl は出力の項目を足す。知らないキーで落ちてはいけない。
    #[test]
    fn unknown_keys_do_not_break_the_reader() {
        let json = r#"[[2, {"tweet_id":20,"date":"2006-03-21 20:50:14","content":"x",
                            "some_new_field":{"nested":true},"view_count":99}]]"#;
        assert!(parse_timeline(json).is_ok());
    }

    /// 出力形式が変わったら、**読み取りの層だけ**が落ちる。
    #[test]
    fn a_changed_output_shape_is_a_parse_error() {
        for broken in [
            "not json",
            r#"[[2, {"date":"2006-03-21 20:50:14","content":"x"}]]"#, // tweet_id が無い
            r#"[[2, {"tweet_id":20,"content":"x"}]]"#,                // 日時が無い
            r#"[[2, {"tweet_id":20,"date":"きのう","content":"x"}]]"#, // 日時が読めない
            r#"[[3, "https://x/a.jpg", {}]]"#,                        // 見出しの前にファイル
        ] {
            assert!(
                matches!(parse_timeline(broken), Err(AdapterError::Parse { .. })),
                "{broken}"
            );
        }
    }

    /// 実物から取った出力。cookie 無しでタイムラインを叩いたときのもの。
    ///
    /// **失敗が標準エラーではなく出力の中に混ざる。** 終了コードだけ見ていると
    /// 「空のタイムライン」に見えてしまう。
    const AUTH_REQUIRED: &str = include_str!("../tests/fixtures/gallerydl/auth-required.json");

    #[test]
    fn an_auth_failure_inside_the_output_is_not_an_empty_timeline() {
        let error = parse_timeline(AUTH_REQUIRED).expect_err("失敗として読めること");

        assert!(
            matches!(error, AdapterError::Unauthenticated { .. }),
            "{error:?}"
        );
        // cookie の失効は消えたことを意味しない。判定は保留（#5）。
        assert_eq!(error.presence(), escrow_domain::liveness::Presence::Unknown);
    }

    #[test]
    fn other_failures_in_the_output_are_transient() {
        let json = r#"[[-1, {"error": "HttpError", "message": "503 Service Unavailable"}]]"#;
        let error = parse_timeline(json).unwrap_err();

        assert!(matches!(error, AdapterError::Transient { .. }), "{error:?}");
        assert_eq!(error.presence(), escrow_domain::liveness::Presence::Unknown);
    }

    #[test]
    fn login_problems_are_their_own_failure() {
        let completed = Completed {
            success: false,
            stdout: String::new(),
            stderr: "[twitter][error] Login required to access this resource".to_owned(),
        };
        assert!(matches!(
            classify(&completed),
            AdapterError::Unauthenticated { .. }
        ));
        // cookie の失効は消えたことを意味しない。判定は保留（#5）。
        assert_eq!(
            classify(&completed).presence(),
            escrow_domain::liveness::Presence::Unknown
        );
    }

    // ---- 名前を #1 の規則へ移す ----

    #[test]
    fn downloaded_files_are_renamed_into_our_scheme() {
        let from = tempfile::tempdir().unwrap();
        let into = tempfile::tempdir().unwrap();

        // gallery-dl が付けた名前。
        for name in ["1.jpg", "2.jpg", "3.mp4"] {
            std::fs::write(from.path().join(name), b"x").unwrap();
        }

        let assets = rename_into_place(from.path(), into.path()).unwrap();
        let names: Vec<_> = assets.iter().map(Asset::file_name).collect();

        // 通し番号は種類ごとに1から。
        assert_eq!(names, ["video.1.mp4", "image.1.jpg", "image.2.jpg"]);
        for name in &names {
            assert!(into.path().join(name).is_file(), "{name}");
        }
    }

    /// 知らない拡張子は実体として数えない。
    #[test]
    fn unknown_extensions_are_left_alone() {
        let from = tempfile::tempdir().unwrap();
        let into = tempfile::tempdir().unwrap();
        std::fs::write(from.path().join("1.jpg"), b"x").unwrap();
        std::fs::write(from.path().join("2.part"), b"x").unwrap();

        let assets = rename_into_place(from.path(), into.path()).unwrap();
        assert_eq!(assets.len(), 1);
    }

    /// 出力の形が増えたら気づけること。黙って捨てると取りこぼしが
    /// 「空のタイムライン」に見える。
    #[test]
    fn an_unknown_envelope_kind_is_a_parse_error() {
        let json = r#"[[2, {"tweet_id":20,"date":"2006-03-21 20:50:14","content":"x"}],
                       [99, {"something": "new"}]]"#;

        assert!(matches!(
            parse_timeline(json),
            Err(AdapterError::Parse { .. })
        ));
    }

    #[test]
    fn nothing_downloaded_is_a_parse_error() {
        let from = tempfile::tempdir().unwrap();
        let into = tempfile::tempdir().unwrap();

        assert!(matches!(
            rename_into_place(from.path(), into.path()),
            Err(AdapterError::Parse { .. })
        ));
    }
}

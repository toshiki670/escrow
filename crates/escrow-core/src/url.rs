//! URL の正規化。#1 の「URL の正規化」。
//!
//! 原則は「可変な表示名ではなく、不変の ID へ寄せる」。ハンドル名は改名されうるので、
//! URL に残すと改名の瞬間に同じものが別行になる。
//!
//! ネットワークへ出ない純関数にしてある。`Item.url` の `UNIQUE` が何を同一と見なすかは
//! 行を入れる時点で確定していなければならないため。

use thiserror::Error;
use url::Url;

use crate::content::ContentType;

/// 正規化を通った URL。
///
/// 生の文字列からは作れない。`Item.url` の `UNIQUE` が何を同一と見なすかを、
/// 約束ではなくこの型が担保する。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NormalizedUrl(String);

impl NormalizedUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NormalizedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 入口が種別について何を語っているか。
///
/// 正規形と一緒に返す。`/shorts/<id>` を `/watch?v=<id>` へ潰すと `youtube_shorts` と
/// `youtube_video` を分ける手掛かりが消えるので、**正規化する前**に読む（#1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeHint {
    /// パスが種別を決めている。
    Known(ContentType),
    /// YouTube の `/watch?v=` と `youtu.be/` は shorts / video / live を区別しない。
    /// 検知なら列挙したタブが、人の登録なら別の手段が決める（#7）。
    YoutubeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UrlError {
    #[error("URL として読めない: {0}")]
    Malformed(String),
    #[error("escrow が扱わないホスト: {0}")]
    UnknownHost(String),
    #[error("{host} の URL だが、項目を指していない: {input}")]
    NotAnItem { host: String, input: String },
    #[error("{platform} の配信元 URL は不変 ID へ解決してから渡す: {input}")]
    UnresolvedSource {
        platform: &'static str,
        input: String,
    },
}

/// 項目の URL を正規形へ写し、入口が語る種別を一緒に返す。
///
/// 先に正規化して後から種別を訊く形は作らない（#1 の決め事）。
pub fn normalize_item(input: &str) -> Result<(NormalizedUrl, TypeHint), UrlError> {
    let parsed = parse(input)?;

    match host_kind(&parsed).ok_or_else(|| UrlError::UnknownHost(input.to_owned()))? {
        Host::Youtube => youtube_item(&parsed, input),
        Host::YoutubeShort => youtu_be_item(&parsed, input),
        Host::X => x_item(&parsed, input),
    }
}

/// 配信元の URL を正規形へ写す。
///
/// ハンドルは受け付けない。改名されうるうえ、不変 ID への解決はネットワークを
/// 要る仕事で、この関数の責務ではないため。解決してから渡す。
pub fn normalize_source(input: &str) -> Result<NormalizedUrl, UrlError> {
    let parsed = parse(input)?;

    match host_kind(&parsed).ok_or_else(|| UrlError::UnknownHost(input.to_owned()))? {
        Host::Youtube => match segments(&parsed).as_slice() {
            ["channel", id] if is_channel_id(id) => Ok(NormalizedUrl(format!(
                "https://www.youtube.com/channel/{id}"
            ))),
            _ => Err(UrlError::UnresolvedSource {
                platform: "YouTube",
                input: input.to_owned(),
            }),
        },
        Host::X => x_source(&parsed, input),
        Host::YoutubeShort => Err(UrlError::NotAnItem {
            host: "youtu.be".to_owned(),
            input: input.to_owned(),
        }),
    }
}

enum Host {
    Youtube,
    YoutubeShort,
    X,
}

fn parse(input: &str) -> Result<Url, UrlError> {
    Url::parse(input.trim()).map_err(|_| UrlError::Malformed(input.to_owned()))
}

fn host_kind(url: &Url) -> Option<Host> {
    // ホストの大文字小文字は url crate が畳んでいる。
    match url.host_str()? {
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            Some(Host::Youtube)
        }
        "youtu.be" => Some(Host::YoutubeShort),
        "x.com" | "www.x.com" | "twitter.com" | "www.twitter.com" | "mobile.twitter.com"
        | "mobile.x.com" => Some(Host::X),
        _ => None,
    }
}

fn segments(url: &Url) -> Vec<&str> {
    url.path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).collect())
        .unwrap_or_default()
}

fn query_value<'a>(url: &'a Url, key: &str) -> Option<std::borrow::Cow<'a, str>> {
    url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn youtube_item(url: &Url, input: &str) -> Result<(NormalizedUrl, TypeHint), UrlError> {
    let not_an_item = || UrlError::NotAnItem {
        host: "youtube.com".to_owned(),
        input: input.to_owned(),
    };

    let (id, hint) = match segments(url).as_slice() {
        // 種別を決められる入口。
        ["shorts", id] => (
            (*id).to_owned(),
            TypeHint::Known(ContentType::YoutubeShorts),
        ),
        ["live", id] => ((*id).to_owned(), TypeHint::Known(ContentType::YoutubeLive)),
        // 決められない入口。ショートも配信のアーカイブもここから開ける。
        ["watch"] => (
            query_value(url, "v").ok_or_else(not_an_item)?.into_owned(),
            TypeHint::YoutubeUnknown,
        ),
        _ => return Err(not_an_item()),
    };

    if !is_video_id(&id) {
        return Err(not_an_item());
    }
    Ok((youtube_watch(&id), hint))
}

fn youtu_be_item(url: &Url, input: &str) -> Result<(NormalizedUrl, TypeHint), UrlError> {
    match segments(url).as_slice() {
        [id] if is_video_id(id) => Ok((youtube_watch(id), TypeHint::YoutubeUnknown)),
        _ => Err(UrlError::NotAnItem {
            host: "youtu.be".to_owned(),
            input: input.to_owned(),
        }),
    }
}

fn x_item(url: &Url, input: &str) -> Result<(NormalizedUrl, TypeHint), UrlError> {
    let not_an_item = || UrlError::NotAnItem {
        host: "x.com".to_owned(),
        input: input.to_owned(),
    };

    // ハンドルは改名されうるので落とし、不変の ID だけを残す。
    // x.com/i/status/<id> は 307 で現ハンドルの URL へ飛ぶので、出典としては読める。
    let (kind, id) = match segments(url).as_slice() {
        // /<handle>/status/<id> と、その後ろに /photo/1 などが付く形。
        // 先頭は捨てるので、/i/status/<id> もここに入る。
        [_, "status" | "statuses", id, ..] => ("status", *id),
        // 共有リンクやリダイレクトの途中で出る形。x.com/i/web/status/20 は
        // 307 で x.com/jack/status/20 へ飛ぶ。
        ["i", "web", "status" | "statuses", id, ..] => ("status", *id),
        ["i", "spaces", id, ..] => ("spaces", *id),
        ["i", "broadcasts", id, ..] => ("broadcasts", *id),
        _ => return Err(not_an_item()),
    };

    if !is_x_id(id) {
        return Err(not_an_item());
    }

    let content_type = match kind {
        "status" => ContentType::XPost,
        "spaces" => ContentType::XSpace,
        _ => ContentType::XBroadcast,
    };
    Ok((
        NormalizedUrl(format!("https://x.com/i/{kind}/{id}")),
        TypeHint::Known(content_type),
    ))
}

fn x_source(url: &Url, input: &str) -> Result<NormalizedUrl, UrlError> {
    let unresolved = || UrlError::UnresolvedSource {
        platform: "X",
        input: input.to_owned(),
    };

    // X が数値のユーザー ID で人を指す2つの形。プロフィールへの直接リンクと、
    // フォロー導線が使う intent。どちらもハンドルを含まない。
    let id = match segments(url).as_slice() {
        ["i", "user", id] => (*id).to_owned(),
        ["intent", "user"] => query_value(url, "user_id")
            .ok_or_else(unresolved)?
            .into_owned(),
        // ハンドルは改名されうるので受け付けない。
        _ => return Err(unresolved()),
    };

    if !is_numeric_id(&id) {
        return Err(unresolved());
    }
    Ok(NormalizedUrl(format!("https://x.com/i/user/{id}")))
}

fn youtube_watch(id: &str) -> NormalizedUrl {
    NormalizedUrl(format!("https://www.youtube.com/watch?v={id}"))
}

/// YouTube の動画 ID は 11 文字の base64url。
fn is_video_id(s: &str) -> bool {
    s.len() == 11 && s.bytes().all(is_base64url)
}

/// チャンネル ID は `UC` で始まる 24 文字。
fn is_channel_id(s: &str) -> bool {
    s.len() == 24 && s.starts_with("UC") && s.bytes().all(is_base64url)
}

/// ユーザー ID は数字。
fn is_numeric_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Space とライブ配信の ID は base64url、投稿 ID は数字。まとめて緩く見る。
fn is_x_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(is_base64url)
}

const fn is_base64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同じ動画へ辿り着く入口は、どれも1つの正規形へ潰れる。
    #[test]
    fn youtube_entrances_collapse_to_one_form() {
        const WATCH: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

        let cases = [
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                TypeHint::YoutubeUnknown,
            ),
            (
                "https://youtube.com/watch?v=dQw4w9WgXcQ",
                TypeHint::YoutubeUnknown,
            ),
            (
                "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
                TypeHint::YoutubeUnknown,
            ),
            ("https://youtu.be/dQw4w9WgXcQ", TypeHint::YoutubeUnknown),
            (
                "https://youtu.be/dQw4w9WgXcQ?si=abc123&t=42",
                TypeHint::YoutubeUnknown,
            ),
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLxx&index=3&pp=ygUK",
                TypeHint::YoutubeUnknown,
            ),
            (
                "https://www.youtube.com/shorts/dQw4w9WgXcQ",
                TypeHint::Known(ContentType::YoutubeShorts),
            ),
            (
                "https://www.youtube.com/live/dQw4w9WgXcQ",
                TypeHint::Known(ContentType::YoutubeLive),
            ),
        ];

        for (input, expected_hint) in cases {
            let (url, hint) = normalize_item(input).expect(input);
            assert_eq!(url.as_str(), WATCH, "{input}");
            assert_eq!(hint, expected_hint, "{input}");
        }
    }

    /// ハンドルは落ちる。改名されても同じ行に着くのがこの正規化の目的。
    #[test]
    fn x_posts_drop_the_handle() {
        const CANONICAL: &str = "https://x.com/i/status/20";

        for input in [
            "https://x.com/jack/status/20",
            "https://x.com/someone_else/status/20",
            "https://twitter.com/jack/status/20",
            "https://mobile.twitter.com/jack/status/20",
            "https://x.com/jack/status/20?s=20&t=abc",
            "https://x.com/jack/status/20/photo/1",
            "https://x.com/i/status/20",
            "https://x.com/i/web/status/20",
        ] {
            let (url, hint) = normalize_item(input).expect(input);
            assert_eq!(url.as_str(), CANONICAL, "{input}");
            assert_eq!(hint, TypeHint::Known(ContentType::XPost), "{input}");
        }
    }

    #[test]
    fn x_spaces_and_broadcasts_keep_their_id_form() {
        let (url, hint) = normalize_item("https://x.com/i/spaces/1YpKkZWvaQvGj").unwrap();
        assert_eq!(url.as_str(), "https://x.com/i/spaces/1YpKkZWvaQvGj");
        assert_eq!(hint, TypeHint::Known(ContentType::XSpace));

        let (url, hint) = normalize_item("https://x.com/i/broadcasts/1YpKkZWvaQvGj").unwrap();
        assert_eq!(url.as_str(), "https://x.com/i/broadcasts/1YpKkZWvaQvGj");
        assert_eq!(hint, TypeHint::Known(ContentType::XBroadcast));
    }

    #[test]
    fn rejects_what_it_cannot_canonicalize() {
        for input in [
            "not a url",
            "https://example.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch",
            "https://www.youtube.com/@YouTube/videos",
            "https://youtu.be/",
            "https://x.com/jack",
            "https://x.com/i/spaces/",
        ] {
            assert!(normalize_item(input).is_err(), "通ってはいけない: {input}");
        }
    }

    #[test]
    fn youtube_sources_must_arrive_as_channel_ids() {
        let url = normalize_source("https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ")
            .expect("channel id");
        assert_eq!(
            url.as_str(),
            "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ"
        );

        // `@handle` は改名されうるので、解決前の形は受け付けない。
        assert!(matches!(
            normalize_source("https://www.youtube.com/@YouTube"),
            Err(UrlError::UnresolvedSource { .. })
        ));
    }

    /// X の配信元も不変 ID へ寄せる。X がハンドル抜きで人を指す2つの形を受ける。
    #[test]
    fn x_sources_must_arrive_as_numeric_ids() {
        const CANONICAL: &str = "https://x.com/i/user/12";

        for input in [
            "https://x.com/i/user/12",
            "https://twitter.com/i/user/12",
            "https://x.com/intent/user?user_id=12",
        ] {
            assert_eq!(normalize_source(input).expect(input).as_str(), CANONICAL);
        }

        // ハンドルは改名されうるので、解決前の形は受け付けない。
        for handle in [
            "https://x.com/jack",
            "https://x.com/i/user/jack",
            "https://x.com/intent/user?screen_name=jack",
        ] {
            assert!(
                matches!(
                    normalize_source(handle),
                    Err(UrlError::UnresolvedSource { platform: "X", .. })
                ),
                "{handle}"
            );
        }
    }
}

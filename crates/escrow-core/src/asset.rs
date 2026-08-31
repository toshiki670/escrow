//! 手元の実体の置き場所。
//!
//! `Asset` はテーブルにならない。#1 のクラス図に在ったものが、ここでは
//! ファイルシステムの命名規則になる。
//!
//! ```text
//! <media_dir>/<item_id>/
//!     video.1.mp4        動画。ライブが切れた断片は video.2.mp4, video.3.mp4 …
//!     audio.1.m4a        音声（Space）
//!     image.1.jpg        画像。X 投稿は最大4枚
//!     transcript.1.vtt   文字起こし
//! ```

use std::fmt;
use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use crate::item::ItemId;

/// 実体の種類。ファイル名の先頭に出る。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetKind {
    Video,
    Audio,
    Image,
    Transcript,
}

impl AssetKind {
    pub const ALL: [Self; 4] = [Self::Video, Self::Audio, Self::Image, Self::Transcript];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Image => "image",
            Self::Transcript => "transcript",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// 拡張子から種類を当てる。
    ///
    /// 取得する側が自分の名前で書いたものを、#1 の命名規則へ移すときに使う。
    /// **名前を決めるのは escrow** なので、ツールが何と呼んだかは持ち込まない。
    pub fn of_extension(extension: &str) -> Option<Self> {
        let lower = extension.to_ascii_lowercase();
        match lower.as_str() {
            "mp4" | "webm" | "mkv" | "mov" | "m4v" => Some(Self::Video),
            "m4a" | "mp3" | "aac" | "ogg" | "opus" | "wav" => Some(Self::Audio),
            "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif" => Some(Self::Image),
            "vtt" => Some(Self::Transcript),
            _ => None,
        }
    }
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 手元の実体1つ。`<kind>.<ordinal>.<ext>` と1対1で対応する。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Asset {
    pub kind: AssetKind,
    /// 1 から始まる通し番号。ライブが切れた断片は 2, 3 と増える。
    pub ordinal: NonZeroU32,
    /// 拡張子。取得する側が実際に何を書くかで変わる（mp4 / webm など）ので、
    /// 種類からは決めない。読む側は来たものを受け取る。`.` は含まない。
    pub extension: String,
}

impl Asset {
    pub fn new(kind: AssetKind, ordinal: NonZeroU32, extension: impl Into<String>) -> Self {
        Self {
            kind,
            ordinal,
            extension: extension.into(),
        }
    }

    pub fn file_name(&self) -> String {
        format!("{}.{}.{}", self.kind, self.ordinal, self.extension)
    }

    pub fn path(&self, media_dir: &Path, item: ItemId) -> PathBuf {
        item_dir(media_dir, item).join(self.file_name())
    }

    /// ファイル名から読み戻す。規則に合わないものは `None`。
    ///
    /// ディレクトリには取得中の中間ファイルなど規則外のものも落ちうるので、
    /// この関数はどんな文字列を渡されても落ちない。
    pub fn parse_file_name(file_name: &str) -> Option<Self> {
        // ちょうど3つ。`video.1.mp4.part` のように途中で増えた中間ファイルは
        // 4つに割れるのでここで落ちる。まだ取得中のものを実体として数えない。
        let [kind, ordinal_text, extension] =
            <[&str; 3]>::try_from(file_name.split('.').collect::<Vec<_>>()).ok()?;

        let kind = AssetKind::parse(kind)?;
        if extension.is_empty() {
            return None;
        }

        let ordinal: NonZeroU32 = ordinal_text.parse().ok()?;
        // `01` を 1 として受けると file_name() と往復しなくなる。
        if ordinal.to_string() != ordinal_text {
            return None;
        }

        Some(Self::new(kind, ordinal, extension))
    }
}

/// 1つの項目の実体が入るディレクトリ。
pub fn item_dir(media_dir: &Path, item: ItemId) -> PathBuf {
    media_dir.join(item.to_string())
}

/// 手元にある実体を読み出す。種類、次に通し番号の順に並ぶ。
///
/// ディレクトリが無い場合は空を返す。まだ何も取得していない項目は普通にこれ。
pub fn scan(media_dir: &Path, item: ItemId) -> io::Result<Vec<Asset>> {
    scan_dir(&item_dir(media_dir, item))
}

/// 1つの項目の実体をまとめて消す。
///
/// `discarded` と `released` の後始末。どちらも**台帳を先に更新してからここへ
/// 来る**ので、途中で落ちても残るのは行と対応しない孤児ファイルだけになる（#7）。
///
/// もう無ければ何もしない。前の周の途中で落ちていれば普通にこれ。
pub fn remove(media_dir: &Path, item: ItemId) -> io::Result<()> {
    match fs::remove_dir_all(item_dir(media_dir, item)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// 置き場所を直接指してのぞく。
///
/// 外部ツールのアダプタは `ItemId` を知らず、書き込み先のディレクトリだけを渡される。
pub fn scan_dir(dir: &Path) -> io::Result<Vec<Asset>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut assets: Vec<Asset> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            Asset::parse_file_name(name.to_str()?)
        })
        .collect();

    assets.sort();
    Ok(assets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordinal(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).expect("テストの通し番号は 1 以上")
    }

    #[test]
    fn file_names_follow_the_naming_rule() {
        let cases = [
            (AssetKind::Video, 1, "mp4", "video.1.mp4"),
            (AssetKind::Video, 3, "mp4", "video.3.mp4"),
            (AssetKind::Audio, 1, "m4a", "audio.1.m4a"),
            (AssetKind::Image, 4, "jpg", "image.4.jpg"),
            (AssetKind::Transcript, 1, "vtt", "transcript.1.vtt"),
        ];

        for (kind, n, ext, expected) in cases {
            let asset = Asset::new(kind, ordinal(n), ext);
            assert_eq!(asset.file_name(), expected);
            assert_eq!(Asset::parse_file_name(expected).as_ref(), Some(&asset));
        }
    }

    /// 拡張子は種類から決めない。webm が書かれれば webm で持つ。
    #[test]
    fn extension_comes_from_the_file_not_the_kind() {
        let webm = Asset::parse_file_name("video.1.webm").unwrap();
        assert_eq!(webm.kind, AssetKind::Video);
        assert_eq!(webm.extension, "webm");
    }

    /// 規則外の名前で落ちないこと。ディレクトリには中間ファイルも落ちる。
    #[test]
    fn ignores_names_that_do_not_follow_the_rule() {
        for name in [
            "",
            "video",
            "video.1",
            "video.1.",
            "video.0.mp4",  // 通し番号は 1 から
            "video.01.mp4", // 往復しない形は受けない
            "video.-1.mp4",
            "movie.1.mp4",
            "video.1.mp4.part", // 取得中の中間ファイル
            "transcript.1.ja.vtt",
            ".hidden",
        ] {
            assert!(
                Asset::parse_file_name(name).is_none(),
                "受けてはいけない: {name:?}"
            );
        }
    }

    #[test]
    fn kinds_can_be_guessed_from_an_extension() {
        for (ext, expected) in [
            ("mp4", AssetKind::Video),
            ("WEBM", AssetKind::Video),
            ("m4a", AssetKind::Audio),
            ("jpg", AssetKind::Image),
            ("vtt", AssetKind::Transcript),
        ] {
            assert_eq!(AssetKind::of_extension(ext), Some(expected), "{ext}");
        }

        // 知らないものは当てずっぽうで決めない。
        assert_eq!(AssetKind::of_extension("part"), None);
        assert_eq!(AssetKind::of_extension(""), None);
    }

    #[test]
    fn paths_are_derived_from_the_item_id() {
        let media_dir = Path::new("/Users/t/Movies/escrow");
        let asset = Asset::new(AssetKind::Video, ordinal(1), "mp4");

        assert_eq!(
            item_dir(media_dir, ItemId::new(42)),
            Path::new("/Users/t/Movies/escrow/42")
        );
        assert_eq!(
            asset.path(media_dir, ItemId::new(42)),
            Path::new("/Users/t/Movies/escrow/42/video.1.mp4")
        );
    }

    #[test]
    fn scanning_an_absent_directory_yields_nothing() {
        let media_dir = tempfile::tempdir().unwrap();
        assert!(scan(media_dir.path(), ItemId::new(1)).unwrap().is_empty());
    }

    #[test]
    fn scanning_sorts_by_kind_then_ordinal() {
        let media_dir = tempfile::tempdir().unwrap();
        let item = ItemId::new(42);
        let dir = item_dir(media_dir.path(), item);
        fs::create_dir_all(&dir).unwrap();

        for name in [
            "transcript.2.vtt",
            "video.2.mp4",
            "transcript.1.vtt",
            "video.1.mp4",
            "download.log",     // 規則外
            "video.3.mp4.part", // 取得中の中間ファイル
        ] {
            fs::write(dir.join(name), b"").unwrap();
        }

        let found: Vec<String> = scan(media_dir.path(), item)
            .unwrap()
            .iter()
            .map(Asset::file_name)
            .collect();

        assert_eq!(
            found,
            [
                "video.1.mp4",
                "video.2.mp4",
                "transcript.1.vtt",
                "transcript.2.vtt"
            ]
        );
    }
}

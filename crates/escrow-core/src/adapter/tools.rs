//! 外部ツールを探す。
//!
//! **この解決器は crate に1つだけ置く。** #5 のアダプタが実際に呼ぶ場所と、
//! #2 の設定画面が「どこで見つかったか」を表示する値は、同じものでなければならない。
//! 2か所に書くと、画面の表示と実際の挙動がずれる。
//!
//! 探し方は PATH が先、`tools.extra_paths` が後（#2）。GUI アプリはターミナルと違う
//! PATH で起動される（`.zshrc` を読まない）ので、Homebrew や mise で入れたものを
//! 見つけられないことがある。`extra_paths` はそのときの逃げ道。

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// escrow が呼ぶ外部ツール。
///
/// 一覧の出所は #5 の対応表。#2 の設定画面と #3 の `depends_on` もそこから来る。
/// `ffmpeg` は yt-dlp が内部で呼ぶので escrow は直接叩かないが、入っていないと
/// 取得が失敗するので、見つかるかどうかは確かめる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    YtDlp,
    GalleryDl,
    Ffmpeg,
    WhisperCli,
}

impl Tool {
    pub const ALL: [Self; 4] = [Self::YtDlp, Self::GalleryDl, Self::Ffmpeg, Self::WhisperCli];

    /// PATH の中で探す名前。
    pub const fn program(self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::GalleryDl => "gallery-dl",
            Self::Ffmpeg => "ffmpeg",
            // 提供するのは whisper-cpp formula で、名前が一致しない（#3）。
            Self::WhisperCli => "whisper-cli",
        }
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.program())
    }
}

/// 探した結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Found(PathBuf),
    NotFound,
}

impl Resolution {
    /// 決まったパスにファイルが在るか。文字起こしモデルの確認に使う。
    ///
    /// モデルは PATH では探さない（実行ファイルではなく、設定が場所を指す）が、
    /// 設定画面は同じ形で並べる（#2 の画面）ので、結果の型は共通にする。
    pub fn of_file(path: &Path) -> Self {
        if path.is_file() {
            Self::Found(path.to_owned())
        } else {
            Self::NotFound
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Found(path) => Some(path),
            Self::NotFound => None,
        }
    }

    pub const fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }
}

/// 探す場所を固定した解決器。
///
/// PATH を引数で受けるのは、環境に依らずテストできるようにするため。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolver {
    directories: Vec<PathBuf>,
}

impl Resolver {
    /// PATH と `extra_paths` から作る。順序は PATH が先。
    pub fn new(path_var: Option<&OsStr>, extra_paths: &[PathBuf]) -> Self {
        let mut directories: Vec<PathBuf> = path_var
            .map(|value| std::env::split_paths(value).collect())
            .unwrap_or_default();
        directories.extend(extra_paths.iter().cloned());

        Self { directories }
    }

    /// 実行中のプロセスの PATH から作る。
    pub fn from_env(extra_paths: &[PathBuf]) -> Self {
        Self::new(std::env::var_os("PATH").as_deref(), extra_paths)
    }

    /// 探した場所。#2 の設定画面が `extra_paths` に何を足せばよいか示すために出す。
    pub fn directories(&self) -> &[PathBuf] {
        &self.directories
    }

    pub fn resolve(&self, tool: Tool) -> Resolution {
        self.directories
            .iter()
            .map(|dir| dir.join(tool.program()))
            .find(|candidate| is_executable(candidate))
            .map_or(Resolution::NotFound, Resolution::Found)
    }

    /// #2 の設定画面が一覧で見せるもの。
    pub fn resolve_all(&self) -> Vec<(Tool, Resolution)> {
        Tool::ALL
            .into_iter()
            .map(|tool| (tool, self.resolve(tool)))
            .collect()
    }

    /// 見つからなかったものだけ。取得を始める前の門になる。
    pub fn missing(&self) -> Vec<Tool> {
        Tool::ALL
            .into_iter()
            .filter(|tool| !self.resolve(*tool).is_found())
            .collect()
    }
}

/// **このプロセスが実行できるか**を確かめる。
///
/// 実行ビットが「誰かに」立っているかを自分で見ると、他人にだけ実行を許した
/// ファイル（`0o010` など）を見つけたことにしてしまい、実際に起動して初めて
/// 落ちる。しかも PATH の手前にそれが在ると、後ろの本物を隠す。
///
/// 正しい問いは `access(2)` の `X_OK` だが、std が出すのは `mode` までで、
/// `access` を直接呼ぶには `unsafe` が要る（この crate は `unsafe_code = "forbid"`）。
/// `which` に任せる。絶対パスを渡した場合は PATH を読まず、その1つを調べる。
fn is_executable(path: &Path) -> bool {
    which::which(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    /// 実行できるファイルを1つ置く。
    fn put_executable(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        path
    }

    fn path_var(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn finds_a_tool_on_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let expected = put_executable(dir.path(), "yt-dlp");

        let resolver = Resolver::new(Some(&path_var(&[dir.path()])), &[]);
        assert_eq!(resolver.resolve(Tool::YtDlp), Resolution::Found(expected));
    }

    #[test]
    fn reports_what_it_could_not_find() {
        let dir = tempfile::tempdir().unwrap();
        put_executable(dir.path(), "yt-dlp");

        let resolver = Resolver::new(Some(&path_var(&[dir.path()])), &[]);
        assert_eq!(resolver.resolve(Tool::GalleryDl), Resolution::NotFound);
        assert_eq!(
            resolver.missing(),
            [Tool::GalleryDl, Tool::Ffmpeg, Tool::WhisperCli]
        );
    }

    /// PATH が空のときが、GUI から起動した場合に起きうる状態。
    #[test]
    fn an_absent_path_finds_nothing() {
        let resolver = Resolver::new(None, &[]);
        assert_eq!(resolver.missing().len(), Tool::ALL.len());
    }

    /// `extra_paths` は見つからないときの逃げ道として効く。
    #[test]
    fn extra_paths_cover_what_the_path_misses() {
        let on_path = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        put_executable(on_path.path(), "yt-dlp");
        let rescued = put_executable(extra.path(), "gallery-dl");

        let resolver = Resolver::new(
            Some(&path_var(&[on_path.path()])),
            &[extra.path().to_owned()],
        );

        assert_eq!(
            resolver.resolve(Tool::GalleryDl),
            Resolution::Found(rescued)
        );
    }

    /// 両方に在れば PATH が勝つ。#3 の「PATH の順で解決される」と揃える。
    #[test]
    fn the_path_wins_over_extra_paths() {
        let on_path = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let expected = put_executable(on_path.path(), "yt-dlp");
        put_executable(extra.path(), "yt-dlp");

        let resolver = Resolver::new(
            Some(&path_var(&[on_path.path()])),
            &[extra.path().to_owned()],
        );

        assert_eq!(resolver.resolve(Tool::YtDlp), Resolution::Found(expected));
    }

    /// 名前が合っていても、**このプロセスが実行できなければ**見つけたことにしない。
    ///
    /// `0o010` / `0o001` は「誰かに実行ビットが立っている」が自分では起動できない。
    /// 見つけたことにすると、PATH の後ろに在る本物を隠して実行時に落ちる。
    #[cfg(unix)]
    #[test]
    fn a_file_this_process_cannot_run_does_not_count() {
        use std::os::unix::fs::PermissionsExt;

        for mode in [0o644, 0o444, 0o010, 0o001] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("yt-dlp");
            std::fs::write(&path, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();

            let resolver = Resolver::new(Some(&path_var(&[dir.path()])), &[]);
            assert_eq!(
                resolver.resolve(Tool::YtDlp),
                Resolution::NotFound,
                "mode {mode:o}"
            );
        }
    }

    /// Homebrew や mise が張る symlink は追う。
    #[cfg(unix)]
    #[test]
    fn a_symlink_to_an_executable_counts() {
        use std::os::unix::fs::PermissionsExt;

        let cellar = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();

        let real = cellar.path().join("yt-dlp");
        std::fs::write(&real, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();

        let link = bin.path().join("yt-dlp");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let resolver = Resolver::new(Some(&path_var(&[bin.path()])), &[]);
        assert!(resolver.resolve(Tool::YtDlp).is_found());
    }

    #[test]
    fn a_directory_with_the_right_name_does_not_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("ffmpeg")).unwrap();

        let resolver = Resolver::new(Some(&path_var(&[dir.path()])), &[]);
        assert_eq!(resolver.resolve(Tool::Ffmpeg), Resolution::NotFound);
    }

    #[test]
    fn resolve_all_covers_every_tool() {
        let resolver = Resolver::new(None, &[]);
        let all = resolver.resolve_all();

        assert_eq!(all.len(), Tool::ALL.len());
        assert_eq!(
            all.iter().map(|(tool, _)| *tool).collect::<Vec<_>>(),
            Tool::ALL.to_vec()
        );
    }

    /// モデルは PATH では探さないが、設定画面には同じ形で並ぶ。
    #[test]
    fn a_model_file_resolves_by_its_own_path() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("ggml-large-v3-turbo.bin");
        std::fs::write(&model, b"weights").unwrap();

        assert_eq!(Resolution::of_file(&model), Resolution::Found(model));
        assert_eq!(
            Resolution::of_file(&dir.path().join("missing.bin")),
            Resolution::NotFound
        );
    }
}

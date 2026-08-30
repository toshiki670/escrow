//! 設定。#2 のとおり TOML で持つ。
//!
//! データ構造（#1）に置かないもの — 実行時の振る舞い、外部ツールの在り処、
//! 認証の取得元、ファイルの置き場所 — がここに来る。
//!
//! **設定ファイルの項目と設定画面の項目は一対一に保つ**（#2）。そのため [`Config`] は
//! ファイルに書いてあるとおりの値を持ち、`~` の展開も `db_path` の既定も畳み込まない。
//! 環境と突き合わせて実際の場所を出すのは [`Paths`] の仕事で、下流はそちらだけを見る。

use std::fmt;
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("設定ファイルを読み書きできない: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("設定ファイルを TOML として読めない")]
    Parse(#[from] toml::de::Error),
    #[error("設定を TOML へ書き出せない")]
    Serialize(#[from] toml::ser::Error),
    #[error("設定ファイルの置き場所を決められない（ホームディレクトリが分からない）")]
    NoHome,
}

/// #2 の項目表。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
// 綴り違いを黙って既定へ落とすと、設定したつもりが効かない。はっきり落とす。
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub storage: Storage,
    pub check: Check,
    pub acquire: Acquire,
    pub transcribe: Transcribe,
    pub auth: Auth,
    pub tools: Tools,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Storage {
    /// メディアと文字起こしの置き場所。
    ///
    /// ここだけ Application Support の外に置く。数十〜数百GBになるうえ、
    /// Time Machine やクラウド同期の対象になるため（#2）。
    pub media_dir: String,
    /// DB の場所。空なら Application Support 配下。
    pub db_path: String,
    /// これを下回ったら取得を始めない。
    pub min_free_gb: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Check {
    /// `holding` の項目をまとめて確認する間隔。
    ///
    /// 検知（`Source.discover_interval_minutes`）とは別の概念なので、こちらは共通設定（#1）。
    pub interval_hours: NonZeroU32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Acquire {
    /// これを超えると `error` 状態になる。
    pub max_retries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Transcribe {
    /// whisper.cpp のモデルファイル。
    ///
    /// 同梱せずパスで指す。数GBあり、配布物に含めると Cask が肥大するため（#2）。
    pub model: String,
    pub language: Language,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Auth {
    /// cookie を取り出すブラウザ。
    ///
    /// **生の認証情報は持たない。** cookie 本体をファイルに書かず、取り出し元だけを持つ（#2）。
    /// プラットフォームごとに分けないのは、同じブラウザにログインしているため。
    pub cookies_from: Browser,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tools {
    /// 外部ツールを探すディレクトリ。PATH に足す。
    ///
    /// 見つからないときの逃げ道。GUI アプリはターミナルと違う PATH で起動される
    /// （`.zshrc` を読まない）ので、Homebrew や mise で入れたものを見つけられないことがある（#2）。
    pub extra_paths: Vec<String>,
}

/// 文字起こしの言語。
///
/// 既定は `ja`。対象が日本語の配信なので、`auto` の自動判定より速く取り違えもない（#2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Language {
    /// whisper に判定させる。
    Auto,
    /// 言語コードを指定する。
    Code(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("言語が空")]
pub struct EmptyLanguage;

impl TryFrom<String> for Language {
    type Error = EmptyLanguage;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        // 前後の空白と `auto` の綴り揺れだけ吸収する。言語コードそのものは
        // whisper が解釈するので、escrow は畳まない。
        let trimmed = value.trim();

        if trimmed.is_empty() {
            Err(EmptyLanguage)
        } else if trimmed.eq_ignore_ascii_case("auto") {
            Ok(Self::Auto)
        } else {
            Ok(Self::Code(trimmed.to_owned()))
        }
    }
}

impl From<Language> for String {
    fn from(value: Language) -> Self {
        match value {
            Language::Auto => "auto".to_owned(),
            Language::Code(code) => code,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Code(code) => f.write_str(code),
        }
    }
}

/// cookie の取り出し元。
///
/// 綴りを自由にすると、外部ツールが受け付けない値を設定できてしまう。値は
/// yt-dlp の `--cookies-from-browser` が挙げるものに合わせてある。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Browser {
    Brave,
    Chrome,
    Chromium,
    Edge,
    Firefox,
    Opera,
    Safari,
    Vivaldi,
    Whale,
}

impl Browser {
    pub const ALL: [Self; 9] = [
        Self::Brave,
        Self::Chrome,
        Self::Chromium,
        Self::Edge,
        Self::Firefox,
        Self::Opera,
        Self::Safari,
        Self::Vivaldi,
        Self::Whale,
    ];

    /// 外部ツールへ渡す値。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Brave => "brave",
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Edge => "edge",
            Self::Firefox => "firefox",
            Self::Opera => "opera",
            Self::Safari => "safari",
            Self::Vivaldi => "vivaldi",
            Self::Whale => "whale",
        }
    }
}

impl fmt::Display for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// #2 の既定値の表。
impl Default for Storage {
    fn default() -> Self {
        Self {
            media_dir: "~/Movies/escrow".to_owned(),
            db_path: String::new(),
            min_free_gb: 20,
        }
    }
}

impl Default for Check {
    fn default() -> Self {
        Self {
            interval_hours: NonZeroU32::new(24).expect("24 は 0 ではない"),
        }
    }
}

impl Default for Acquire {
    fn default() -> Self {
        Self { max_retries: 3 }
    }
}

impl Default for Transcribe {
    fn default() -> Self {
        Self {
            model: "~/.local/share/whisper-models/ggml-large-v3-turbo.bin".to_owned(),
            language: Language::Code("ja".to_owned()),
        }
    }
}

impl Default for Auth {
    fn default() -> Self {
        Self {
            cookies_from: Browser::Firefox,
        }
    }
}

impl Config {
    /// 設定ファイルを読む。**無ければ既定を返す。**
    ///
    /// 初回起動でファイルが無いのは異常ではない。壊れている場合だけ落とす。
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };

        Self::from_toml(&text)
    }

    /// 設定画面から書き戻す。**コメントは保持しない**（#2）。
    ///
    /// 同じディレクトリの一時ファイルへ書いてから rename する。途中で落ちても、
    /// 切れた設定ファイルが残らない。[`Config::load`] は無いファイルだけ既定に
    /// 落として壊れたファイルは落とすので、**自分の書き込みでその状態を作らない**
    /// ようにしておく。
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        use std::io::Write as _;

        let text = self.to_toml()?;
        let parent = path.parent().unwrap_or(Path::new("."));

        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| ConfigError::Io { path, source }
        };

        std::fs::create_dir_all(parent).map_err(io_error(parent))?;

        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(io_error(parent))?;
        temp.write_all(text.as_bytes()).map_err(io_error(parent))?;
        // rename が先に見えて中身が後、を防ぐ。
        temp.as_file().sync_all().map_err(io_error(parent))?;
        temp.persist(path).map_err(|e| ConfigError::Io {
            path: path.to_owned(),
            source: e.error,
        })?;

        Ok(())
    }

    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// 外部ツールを探すディレクトリ。`~` は展開済み。
    pub fn extra_paths(&self, dirs: &Dirs) -> Vec<PathBuf> {
        self.tools
            .extra_paths
            .iter()
            .map(|raw| expand(raw, dirs.home()))
            .collect()
    }
}

/// 環境が決める場所。
///
/// `directories` 経由で取るので、macOS では #2 が書いた
/// `~/Library/Application Support/escrow` と同じ値になる。テストでは差し替える。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    home: PathBuf,
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl Dirs {
    pub fn discover() -> Result<Self, ConfigError> {
        let project =
            directories::ProjectDirs::from("", "", "escrow").ok_or(ConfigError::NoHome)?;
        // ホームは std が答える。`directories` は Application Support の
        // 場所を出すためだけに使う。
        let home = std::env::home_dir().ok_or(ConfigError::NoHome)?;

        Ok(Self {
            home,
            config_dir: project.config_dir().to_owned(),
            data_dir: project.data_dir().to_owned(),
        })
    }

    pub fn new(home: PathBuf, config_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            home,
            config_dir,
            data_dir,
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// 設定ファイルの場所。**設定で変えられない**（読むために場所が要るため、#2）。
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
}

/// 設定を環境と突き合わせて出した実際の場所。
///
/// `~` は展開済みで、空の `db_path` も埋まっている。下流はここだけを見るので、
/// 「展開を忘れる」「空文字列のまま渡す」が起きない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_file: PathBuf,
    pub db: PathBuf,
    pub media_dir: PathBuf,
    pub transcribe_model: PathBuf,
}

impl Paths {
    pub fn resolve(config: &Config, dirs: &Dirs) -> Self {
        let db = if config.storage.db_path.trim().is_empty() {
            dirs.data_dir.join("escrow.db")
        } else {
            expand(&config.storage.db_path, dirs.home())
        };

        Self {
            config_file: dirs.config_file(),
            db,
            media_dir: expand(&config.storage.media_dir, dirs.home()),
            transcribe_model: expand(&config.transcribe.model, dirs.home()),
        }
    }
}

/// 設定に書かれたパスを実際の場所へ写す。
///
/// 先頭の1成分が `~` のときだけホームへ差し替える。`~` はシェルの記法で
/// [`std::path`] の概念ではないので、その一段だけがここの仕事。
///
/// **成分の切り出しは [`Path::components`] に任せる。** 区切りが何文字か、
/// 重複した区切りをどう畳むか、`~other` が別の成分かは、すべてそちらが答える。
/// 自分で文字列を切ると環境差が入り込む。
///
/// **相対パスはホーム基準にする。** プロセスの CWD を基準にすると、ターミナルから
/// 起動したときと GUI から起動したときで同じ設定が別の場所を指す。#2 の既定は
/// どれもホーム配下なので、ホーム基準の方が読みとしても自然。
fn expand(raw: &str, home: &Path) -> PathBuf {
    let path = Path::new(raw);

    let mut rest = path.components();
    if matches!(rest.next(), Some(Component::Normal(first)) if first == "~") {
        return home.join(rest.as_path());
    }

    if path.is_absolute() {
        path.to_owned()
    } else {
        home.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2 の項目表に載っている TOML そのもの。
    const EXAMPLE: &str = r#"
[storage]
media_dir = "~/Movies/escrow"
db_path = ""
min_free_gb = 20

[check]
interval_hours = 24

[acquire]
max_retries = 3

[transcribe]
model = "~/.local/share/whisper-models/ggml-large-v3-turbo.bin"
language = "ja"

[auth]
cookies_from = "firefox"

[tools]
extra_paths = []
"#;

    fn dirs() -> Dirs {
        Dirs::new(
            PathBuf::from("/Users/t"),
            PathBuf::from("/Users/t/Library/Application Support/escrow"),
            PathBuf::from("/Users/t/Library/Application Support/escrow"),
        )
    }

    /// #2 が書いた既定値と、この実装の既定が一致すること。
    #[test]
    fn the_example_in_the_issue_is_the_default() {
        assert_eq!(Config::from_toml(EXAMPLE).unwrap(), Config::default());
    }

    /// 設定ファイルが無い状態で既定が入る。初回起動はこれ。
    #[test]
    fn a_missing_file_yields_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    /// 設定画面からの書き戻しが往復する。
    #[test]
    fn writing_back_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");

        let mut config = Config::default();
        config.storage.min_free_gb = 50;
        config.check.interval_hours = NonZeroU32::new(6).unwrap();
        config.transcribe.language = Language::Auto;
        config.auth.cookies_from = Browser::Safari;
        config.tools.extra_paths = vec!["/opt/homebrew/bin".to_owned()];

        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    /// 綴り違いは黙って既定へ落とさない。設定したつもりが効かないのを防ぐ。
    #[test]
    fn a_misspelled_key_is_refused() {
        let typo = r#"
[storage]
media_dirs = "~/Movies/escrow"
"#;
        assert!(Config::from_toml(typo).is_err());
    }

    /// 間隔に 0 は無い。`NonZeroU32` なので読む時点で落ちる。
    #[test]
    fn a_zero_interval_is_refused() {
        let zero = r#"
[check]
interval_hours = 0
"#;
        assert!(Config::from_toml(zero).is_err());
    }

    #[test]
    fn language_keeps_auto_apart_from_a_code() {
        assert_eq!(
            Language::try_from("auto".to_owned()).unwrap(),
            Language::Auto
        );
        assert_eq!(
            Language::try_from("ja".to_owned()).unwrap(),
            Language::Code("ja".to_owned())
        );
        assert!(Language::try_from(String::new()).is_err());
    }

    #[test]
    fn browsers_round_trip_through_their_wire_value() {
        for browser in Browser::ALL {
            let toml = format!("[auth]\ncookies_from = \"{browser}\"\n");
            let config = Config::from_toml(&toml).unwrap();
            assert_eq!(config.auth.cookies_from, browser);
        }

        assert!(Config::from_toml("[auth]\ncookies_from = \"netscape\"\n").is_err());
    }

    /// 空の `db_path` は Application Support 配下へ埋まる。
    #[test]
    fn an_empty_db_path_falls_back_to_the_data_dir() {
        let paths = Paths::resolve(&Config::default(), &dirs());

        assert_eq!(
            paths.db,
            Path::new("/Users/t/Library/Application Support/escrow/escrow.db")
        );
        assert_eq!(
            paths.config_file,
            Path::new("/Users/t/Library/Application Support/escrow/config.toml")
        );
    }

    #[test]
    fn tildes_are_expanded_once_at_the_edge() {
        let paths = Paths::resolve(&Config::default(), &dirs());

        assert_eq!(paths.media_dir, Path::new("/Users/t/Movies/escrow"));
        assert_eq!(
            paths.transcribe_model,
            Path::new("/Users/t/.local/share/whisper-models/ggml-large-v3-turbo.bin")
        );
    }

    #[test]
    fn an_explicit_db_path_wins() {
        let mut config = Config::default();
        config.storage.db_path = "~/somewhere/escrow.db".to_owned();

        let paths = Paths::resolve(&config, &dirs());
        assert_eq!(paths.db, Path::new("/Users/t/somewhere/escrow.db"));
    }

    #[test]
    fn expansion_only_touches_a_leading_tilde() {
        let home = Path::new("/Users/t");

        assert_eq!(expand("~", home), Path::new("/Users/t"));
        assert_eq!(expand("~/Movies", home), Path::new("/Users/t/Movies"));
        assert_eq!(
            expand("/opt/homebrew/bin", home),
            Path::new("/opt/homebrew/bin")
        );
        // 先頭の成分でなければ `~` は普通の名前。
        assert_eq!(expand("/tmp/~/x", home), Path::new("/tmp/~/x"));
    }

    /// 区切りの重複や `.` は [`Path::components`] が畳む。自分では扱わない。
    #[test]
    fn the_path_parser_normalizes_the_rest() {
        let home = Path::new("/Users/t");

        assert_eq!(expand("~//Movies", home), Path::new("/Users/t/Movies"));
        assert_eq!(expand("~/./Movies", home), Path::new("/Users/t/Movies"));
    }

    /// この環境で `\` が区切りかどうかも [`Path::components`] が答える。
    /// Unix では普通のファイル名文字なので、`~\Movies` は1つの成分。
    #[cfg(unix)]
    #[test]
    fn a_backslash_is_an_ordinary_character_here() {
        let home = Path::new("/Users/t");

        assert_eq!(expand("~\\Movies", home), Path::new("/Users/t/~\\Movies"));
        assert_eq!(expand("/tmp/a\\b", home), Path::new("/tmp/a\\b"));
    }

    /// 相対パスは CWD ではなくホームを基準にする。
    ///
    /// CWD 基準にすると、ターミナルから起動したときと GUI から起動したときで
    /// 同じ設定が別の場所を指す。
    #[test]
    fn relative_paths_hang_off_home_not_the_working_directory() {
        let home = Path::new("/Users/t");

        assert_eq!(
            expand("Movies/escrow", home),
            Path::new("/Users/t/Movies/escrow")
        );
        // 別人のホームを指す記法も、escrow は解釈せずホーム基準の相対として扱う。
        assert_eq!(expand("~other/x", home), Path::new("/Users/t/~other/x"));
    }

    /// 一部のセクションだけ書いた設定が、残りの既定と混ざること。
    /// `serde(default)` の契約を固定しておく。
    #[test]
    fn a_partial_file_merges_with_the_defaults() {
        let partial = r#"
[storage]
min_free_gb = 50
"#;
        let config = Config::from_toml(partial).unwrap();

        assert_eq!(config.storage.min_free_gb, 50);
        // 同じセクションの他の項目も、別のセクションも既定のまま。
        assert_eq!(config.storage.media_dir, Storage::default().media_dir);
        assert_eq!(config.transcribe, Transcribe::default());
        assert_eq!(config.auth, Auth::default());
    }

    /// 書き戻しは一時ファイル経由。中途半端なファイルも書き残しも残らない。
    #[test]
    fn saving_leaves_nothing_but_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        Config::default().save(&path).unwrap();
        let mut config = Config::default();
        config.storage.min_free_gb = 99;
        config.save(&path).unwrap();

        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["config.toml"], "一時ファイルが残っていない");
        assert_eq!(Config::load(&path).unwrap().storage.min_free_gb, 99);
    }

    /// `auto` の綴り揺れと前後の空白は吸収する。言語コードそのものは畳まない。
    #[test]
    fn the_auto_sentinel_tolerates_spelling() {
        for raw in ["auto", "AUTO", "Auto", "  auto  "] {
            assert_eq!(
                Language::try_from(raw.to_owned()).unwrap(),
                Language::Auto,
                "{raw:?}"
            );
        }

        assert_eq!(
            Language::try_from("  ja  ".to_owned()).unwrap(),
            Language::Code("ja".to_owned())
        );
        assert!(Language::try_from("   ".to_owned()).is_err());
    }

    #[test]
    fn extra_paths_are_expanded_too() {
        let mut config = Config::default();
        config.tools.extra_paths = vec!["~/bin".to_owned(), "/opt/homebrew/bin".to_owned()];

        assert_eq!(
            config.extra_paths(&dirs()),
            [
                PathBuf::from("/Users/t/bin"),
                PathBuf::from("/opt/homebrew/bin")
            ]
        );
    }
}

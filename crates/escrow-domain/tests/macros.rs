//! `macro_rules!` の一覧を表で固定する。
//!
//! マクロは自由度が高いぶん、書き手の想定を外れた展開が起きてもコンパイラが助けない。
//! 導出・generics・trait で書ける形はそちらを採る、というのが方針で、**足すには下の表を
//! 編集することになる**ので必ず差分に現れる。
//!
//! 置き場所が `escrow-domain` なのは、この crate が誰にも依存されずに残る唯一の段
//! だから。表が見るのは他の crate のファイルだけなので、ここに置いても依存は増えない。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 書いてよい `macro_rules!`。`<crate>/<src からの相対パス>:<名前>` の形で並べる。
///
/// **いまは空。** 識別子の newtype を作る `id_type!` が居たが、`derive_more` の導出に
/// 置き換わって消えた。空のまま残しておくと、1つ目を足すときにも表の編集が要る。
const ALLOWED: &[&str] = &[];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<name>/ の2つ上がワークスペース")
        .to_owned()
}

/// `crates/` 以下の `.rs` を、ワークスペース相対のパスと中身で返す。
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, found: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("読めるエントリ").path();
            if path.is_dir() {
                walk(&path, root, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path
                    .strip_prefix(root)
                    .expect("root の下")
                    .to_string_lossy()
                    .into_owned();
                found.push((name, std::fs::read_to_string(&path).expect("読める")));
            }
        }
    }

    let root = workspace_root().join("crates");
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found.sort();
    found
}

/// `macro_rules! <name>` の宣言を、`<パス>:<名前>` の形で集める。
fn declarations(path: &str, body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim_start)
        .filter_map(|line| line.strip_prefix("macro_rules! "))
        .filter_map(|rest| rest.split([' ', '{']).next())
        .map(|name| format!("{path}:{name}"))
        .collect()
}

#[test]
fn only_the_listed_macros_exist() {
    let actual: BTreeSet<String> = sources()
        .iter()
        .flat_map(|(path, body)| declarations(path, body))
        .collect();
    let allowed: BTreeSet<String> = ALLOWED.iter().map(|s| (*s).to_owned()).collect();

    assert_eq!(actual, allowed, "表に無い macro_rules! がある");
}

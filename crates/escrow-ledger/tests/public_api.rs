//! 公開 API を表で固定する（#15）。
//!
//! `escrow-domain` のモジュール一覧と同じ形。用途に紐づく関数を足そうとすると
//! 表の編集が要るので、**必ず差分に現れる**。
//!
//! そのうえで #7 の受け入れ「投影を直接書き換える関数が無いこと」を、名前ではなく
//! **SQL の置き場所**で確かめる。投影へ書く文が追記と作り直しの2ファイルにしか
//! 無い限り、ログと投影がずれる書き方は存在しない。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `escrow-ledger` が外へ出す関数。**これで全部。**
///
/// 書く道は `discover` と `append` の2つだけで、投影を名指しで動かすものは無い。
/// `rebuild` はログから作り直すので、書くのは投影だが**決めるのはログ**。
const PUBLIC_API: &[&str] = &[
    // 接続
    "open",
    "open_in_memory",
    // 事象を書く
    "discover",
    "append",
    // 投影を読む
    "item",
    "item_by_url",
    "items_in_state",
    // ログを読む
    "log",
    "replay",
    "failures_since_the_state_moved",
    // 投影を作り直す
    "rebuild",
    // 設定（catalog）
    "add_person",
    "person",
    "add_source",
    "source",
    "add_exclude",
    "excludes",
    // Seq
    "get",
    "next",
];

/// 投影へ書く SQL を置いてよいファイル。
const WRITES_THE_PROJECTION: &[&str] = &["item/append.rs", "rebuild.rs"];

/// 投影へ書く文の見分け方。`item_event` に当たらないよう、後ろまで見る。
const WRITES: &[&str] = &[
    "INTO item (",
    "UPDATE item ",
    "DELETE FROM item",
    "DROP TABLE IF EXISTS item",
];

fn src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// `src/` 以下の `.rs` を、crate 相対のパスと**本体だけ**の中身で返す。
///
/// 末尾の `#[cfg(test)] mod tests` から先は落とす。投影を壊してから作り直す確認の
/// ように、テストは投影へ直接書くことがある。見たいのは出荷される経路のほう。
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, found: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.expect("読めるエントリ").path();
            if path.is_dir() {
                walk(&path, root, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path
                    .strip_prefix(root)
                    .expect("root の下")
                    .to_string_lossy()
                    .into_owned();
                let body = std::fs::read_to_string(&path).expect("読める");
                let body = body
                    .split_once("\n#[cfg(test)]\nmod tests {")
                    .map_or(body.as_str(), |(before, _)| before)
                    .to_owned();
                found.push((name, body));
            }
        }
    }

    let root = src();
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found.sort();
    found
}

/// `pub fn` / `pub async fn` / `pub const fn` の名前。
///
/// `pub(crate)` は前置きが違うので当たらない。外へ出るものだけが集まる。
fn public_functions(body: &str) -> BTreeSet<String> {
    body.lines()
        .map(str::trim_start)
        .filter_map(|line| {
            ["pub fn ", "pub async fn ", "pub const fn "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
        })
        .filter_map(|rest| rest.split(['(', '<']).next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn only_the_listed_functions_are_public() {
    let mut actual = BTreeSet::new();
    for (_, body) in sources() {
        actual.extend(public_functions(&body));
    }

    let expected: BTreeSet<String> = PUBLIC_API.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(actual, expected, "表に無い関数が公開されている（#15）");
}

/// 接続そのものは外へ出さない。
///
/// 出すと、表に載っていない SQL をどこからでも書けるようになり、上の表が
/// 「そこを通れば」の話にしかならなくなる。
#[test]
fn the_pool_never_leaves_the_crate() {
    assert!(
        !PUBLIC_API.contains(&"pool"),
        "接続を外へ出すと、投影を直接書ける経路ができる"
    );
}

/// 投影へ書く SQL が、追記と作り直しの2ファイルにしか無いこと（#7 の受け入れ）。
#[test]
fn only_appending_and_rebuilding_touch_the_projection() {
    let allowed: BTreeSet<&str> = WRITES_THE_PROJECTION.iter().copied().collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (name, body) in sources() {
        // このテスト自身が持つ見本の文字列と混ざらないよう、src/ だけを見ている。
        if WRITES.iter().any(|write| body.contains(write)) {
            seen.insert(name.clone());
            assert!(
                allowed.contains(name.as_str()),
                "{name} が投影へ直接書いている。書く道は追記と作り直しだけ（#15）"
            );
        }
    }

    assert_eq!(
        seen,
        allowed.iter().map(|s| (*s).to_owned()).collect(),
        "投影へ書くファイルが減っている。表が古い"
    );
}

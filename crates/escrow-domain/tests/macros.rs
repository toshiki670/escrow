//! **ワークスペース全体で `macro_rules!` を禁じる。**
//!
//! マクロは自由度が高いぶん、書き手の想定を外れた展開が起きてもコンパイラが助けない。
//! `derive`・generics・trait で書ける形は、そちらを採る（`CONTRIBUTING.md`）。
//!
//! # 本当に要るものが出てきたら
//!
//! ここを許可リストへ変える前に、**escrow から独立した crate にできないか**を見る。
//! マクロで解くほど一般的な仕組みなら、escrow に閉じている理由がないことが多い。
//!
//! それでも escrow の中に要るなら、この禁止を「限定的な許可」へ書き換える。そのとき
//! 何を許したかと理由がここに残り、差分に現れる。**いま許可の枠が無いので、1つ目を
//! 入れるにも同じ手間がかかる。**
//!
//! # 置き場所
//!
//! `escrow-domain` に在るのは、この crate が誰にも依存されずに残る唯一の段だから。
//! 見るのは他の crate のファイルだけなので、ここに置いても依存は増えない。走査が
//! ワークスペースの全 crate に届いていることは、下の2つ目のテストが確かめる。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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

#[test]
fn macro_rules_is_banned_across_the_workspace() {
    let declared: Vec<String> = sources()
        .iter()
        .flat_map(|(path, body)| {
            body.lines()
                .map(str::trim_start)
                .filter_map(|line| line.strip_prefix("macro_rules! "))
                .filter_map(|rest| rest.split([' ', '{']).next())
                .map(|name| format!("{path}:{name}"))
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        declared.is_empty(),
        "macro_rules! は禁止。独立した crate に切り出すか、\
         ここの禁止を限定的な許可へ書き換える: {declared:?}"
    );
}

/// 禁止がワークスペースの全 crate に届いていること。
///
/// 走査は `crates/` をディレクトリごと辿るので、そこに在る限り自動で入る。**そこから
/// 外れた member が居ないこと**をここで確かめる。1つでも外に置かれると、その crate だけ
/// マクロを書ける場所になる。
#[test]
fn the_ban_reaches_every_crate_in_the_workspace() {
    let root = workspace_root();
    let manifest: toml::Table = std::fs::read_to_string(root.join("Cargo.toml"))
        .expect("ワークスペースの Cargo.toml")
        .parse()
        .expect("読める TOML");

    let members: BTreeSet<String> = manifest["workspace"]["members"]
        .as_array()
        .expect("workspace.members は配列")
        .iter()
        .map(|member| member.as_str().expect("member は文字列").to_owned())
        .collect();

    let outside: Vec<&String> = members
        .iter()
        .filter(|member| !member.starts_with("crates/"))
        .collect();

    assert!(
        outside.is_empty(),
        "crates/ の外に member が居るので、走査が届かない: {outside:?}"
    );

    // 走査が空振りしていないことも見る。ディレクトリ名を間違えると全部素通りする。
    let scanned: BTreeSet<String> = sources()
        .iter()
        .filter_map(|(path, _)| path.split('/').next().map(str::to_owned))
        .collect();
    let expected: BTreeSet<String> = members
        .iter()
        .filter_map(|m| m.strip_prefix("crates/").map(str::to_owned))
        .collect();

    assert_eq!(scanned, expected, "走査した crate と member が食い違う");
}

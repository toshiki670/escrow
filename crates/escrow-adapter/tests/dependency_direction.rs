//! 依存の向きが #3 の一方通行になっていること。
//!
//! #13 の「外部アクセスはすべて1か所を通す」を守らせているのは規約ではなく
//! **crate の分け方**で、`escrow-adapter` を依存に持つのが `escrow-scheduler` だけ
//! である限り、迂回はコンパイルエラーになる。その前提をここで固定する。
//!
//! 見るのは `Cargo.toml` で、**dev-dependencies も含める**。テストの中でだけ
//! 迂回できるなら、迂回路は在ることになる。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// #3 の依存図。左が右を依存に持ってよい、の全部。
///
/// 実際に使っているかは問わない（`escrow-gui` はまだ空）。ここに無い辺が
/// 生えたら落ちる。
const ALLOWED: &[(&str, &[&str])] = &[
    ("escrow-core", &[]),
    ("escrow-adapter", &["escrow-core"]),
    ("escrow-scheduler", &["escrow-core", "escrow-adapter"]),
    ("escrow-cli", &["escrow-core", "escrow-scheduler"]),
    ("escrow-gui", &["escrow-core", "escrow-scheduler"]),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<name>/ の2つ上がワークスペース")
        .to_owned()
}

fn manifest(path: &Path) -> toml::Table {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.parse()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// その crate が名前を知っている escrow-* の crate。
fn escrow_dependencies_of(crate_name: &str) -> BTreeSet<String> {
    let path = workspace_root()
        .join("crates")
        .join(crate_name)
        .join("Cargo.toml");
    let manifest = manifest(&path);

    ["dependencies", "dev-dependencies", "build-dependencies"]
        .iter()
        .filter_map(|table| manifest.get(*table))
        .filter_map(toml::Value::as_table)
        .flat_map(toml::Table::keys)
        .filter(|name| name.starts_with("escrow-"))
        .cloned()
        .collect()
}

/// #3 の図に無い辺が生えていないこと。
#[test]
fn dependencies_follow_the_one_way_graph() {
    for (crate_name, allowed) in ALLOWED {
        let allowed: BTreeSet<&str> = allowed.iter().copied().collect();

        for actual in escrow_dependencies_of(crate_name) {
            assert!(
                allowed.contains(actual.as_str()),
                "{crate_name} が {actual} に依存している。#3 の向きに無い"
            );
        }
    }
}

/// 外部ツールの名前を知ってよいのは `escrow-scheduler` だけ（#3）。
#[test]
fn only_the_scheduler_knows_the_adapter() {
    for (crate_name, _) in ALLOWED {
        let knows = escrow_dependencies_of(crate_name).contains("escrow-adapter");

        assert_eq!(
            knows,
            *crate_name == "escrow-scheduler",
            "{crate_name} から escrow-adapter への依存"
        );
    }
}

/// 表がワークスペースの全 crate を覆っていること。
///
/// 覆っていないと、6つ目の crate が誰にも見られずに迂回路を作れる。
#[test]
fn the_graph_covers_every_crate_in_the_workspace() {
    let root = workspace_root();
    let manifest = manifest(&root.join("Cargo.toml"));

    let members: BTreeSet<String> = manifest["workspace"]["members"]
        .as_array()
        .expect("workspace.members は配列")
        .iter()
        .map(|member| {
            member
                .as_str()
                .expect("member は文字列")
                .rsplit('/')
                .next()
                .expect("crates/<name>")
                .to_owned()
        })
        .collect();

    let covered: BTreeSet<String> = ALLOWED.iter().map(|(name, _)| (*name).to_owned()).collect();

    assert_eq!(members, covered);
}

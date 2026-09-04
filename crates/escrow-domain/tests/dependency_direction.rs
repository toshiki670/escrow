//! 依存の向きが #3 の一方通行になっていること。
//!
//! #13 の「外部アクセスはすべて1か所を通す」を守らせているのは規約ではなく
//! **crate の分け方**で、外部ツールの crate を依存に持つのが `escrow-scheduler` だけ
//! である限り、迂回はコンパイルエラーになる。その前提をここで固定する。
//!
//! 見るのは `Cargo.toml` で、**dev-dependencies も含める**。テストの中でだけ
//! 迂回できるなら、迂回路は在ることになる。
//!
//! 置き場所が `escrow-domain` なのは、この crate が誰にも依存されずに残る唯一の
//! 段だから。表が見るのは他の `Cargo.toml` だけなので、ここに置いても依存は増えない。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// #3 の依存図。左が右を依存に持ってよい、の全部。
///
/// 実際に使っているかは問わない（`escrow-gui` はまだ空）。ここに無い辺が
/// 生えたら落ちる。
/// スライスが依存してよいもの。互いの名前はここに無い。
const SLICE: &[&str] = &["escrow-domain", "escrow-ledger", "escrow-scheduler"];

/// 入口が依存してよいもの。**緩めて「全部」にしない** — 緩めた瞬間に段5 の検査が
/// 消え、`escrow-external` を直接呼ぶ経路が生える。
const ENTRY: &[&str] = &[
    "escrow-domain",
    "escrow-ledger",
    "escrow-config",
    "escrow-scheduler",
    "escrow-discovery",
    "escrow-acquisition",
    "escrow-transcription",
    "escrow-custody",
    "escrow-handover",
];

const ALLOWED: &[(&str, &[&str])] = &[
    // 段1 — カーネル。誰にも依存しない。
    ("escrow-domain", &[]),
    // 段2 — 設定・永続化・外部ツール。config だけは external と入口が読む。
    ("escrow-config", &[]),
    ("escrow-ledger", &["escrow-domain"]),
    ("escrow-external", &["escrow-domain", "escrow-config"]),
    // 段3 — 外部アクセスの受付。external を依存に持つ唯一の crate（#3）。
    (
        "escrow-scheduler",
        &["escrow-domain", "escrow-config", "escrow-external"],
    ),
    // 段4 — スライス。**同じ段の中も見えない**。handover だけは外へ出ないので
    // スケジューラも要らない（#15）。
    ("escrow-discovery", SLICE),
    ("escrow-acquisition", SLICE),
    ("escrow-transcription", SLICE),
    ("escrow-custody", SLICE),
    ("escrow-handover", &["escrow-domain", "escrow-ledger"]),
    // 段5 — 入口。すべてを組み立てるが、external だけは名前で知らない。
    ("escrow-cli", ENTRY),
    ("escrow-gui", ENTRY),
];

/// 外部ツールを呼ぶ crate。`escrow-scheduler` 以外は名前も知ってはいけない（#3）。
///
/// 定数で持つのは、crate を改名したときに次のテストが**黙って通る**のを防ぐため。
/// 名前が実在することを先に確かめる。
const EXTERNAL: &str = "escrow-external";

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
fn only_the_scheduler_knows_the_external_tools() {
    assert!(
        ALLOWED.iter().any(|(name, _)| *name == EXTERNAL),
        "{EXTERNAL} がワークスペースに無い。改名したなら EXTERNAL も直す"
    );

    for (crate_name, _) in ALLOWED {
        let knows = escrow_dependencies_of(crate_name).contains(EXTERNAL);

        assert_eq!(
            knows,
            *crate_name == "escrow-scheduler",
            "{crate_name} から {EXTERNAL} への依存"
        );
    }
}

/// 表がワークスペースの全 crate を覆っていること。
///
/// 覆っていないと、表に載らない crate が誰にも見られずに迂回路を作れる。
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

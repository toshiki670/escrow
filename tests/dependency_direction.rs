//! 依存の向きが #3 の一方通行になっていること。
//!
//! #13 の「外部アクセスはすべて1か所を通す」を守らせているのは規約ではなく
//! **crate の分け方**で、外部ツールの crate を依存に持つのが `escrow-scheduler` だけ
//! である限り、迂回はコンパイルエラーになる。その前提をここで固定する。
//!

use std::collections::BTreeSet;

use escrow_tests::members;

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

/// 依存の図。左が右を依存に持ってよい、の全部。
///
/// 実際に使っているかは問わない。ここに無い辺が生えたら落ちる。
const ALLOWED: &[(&str, &[&str])] = &[
    // ワークスペース全体にかかる守り。crate を1つも依存に持たない。
    ("escrow-tests", &[]),
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

/// #3 の図に無い辺が生えていないこと。
#[test]
fn dependencies_follow_the_one_way_graph() {
    let allowed: std::collections::BTreeMap<&str, BTreeSet<&str>> = ALLOWED
        .iter()
        .map(|(name, deps)| (*name, deps.iter().copied().collect()))
        .collect();

    for member in members() {
        let allowed = &allowed[member.name.as_str()];

        for actual in member.escrow_dependencies() {
            assert!(
                allowed.contains(actual.as_str()),
                "{} が {actual} に依存している。#3 の向きに無い",
                member.name
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

    for member in members() {
        let knows = member.escrow_dependencies().contains(EXTERNAL);

        assert_eq!(
            knows,
            member.name == "escrow-scheduler",
            "{} から {EXTERNAL} への依存",
            member.name
        );
    }
}

/// 表がワークスペースの全 crate を覆っていること。
///
/// 覆っていないと、表に載らない crate が誰にも見られずに迂回路を作れる。名前は
/// ディレクトリではなく `package.name` から取るので、置き場所を変えても追える。
#[test]
fn the_graph_covers_every_crate_in_the_workspace() {
    let actual: BTreeSet<String> = members().into_iter().map(|m| m.name).collect();
    let covered: BTreeSet<String> = ALLOWED.iter().map(|(name, _)| (*name).to_owned()).collect();

    assert_eq!(actual, covered);
}

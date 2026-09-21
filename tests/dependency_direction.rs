//! 依存の向きが crate の置き場所から出ること（#101）。
//!
//! 最上位は `app/`（Composition Root と、それが組み立てる crate）と `ui/`（入口）。
//! `escrow-app` の配下は入口を知らず、入口は `escrow-app` しか知らない（#82）。`app/crates/`
//! の中では、子（`scheduler/external`）を依存に持てるのは親だけ、`slices/` の中は互いを知らない
//! （#15）。横並びの `event-store` / `config` / `scheduler` / `external` / スライスの間だけは
//! 構造から決まらないので、その分を [`SIDE_BY_SIDE`] の表で持つ。
//!
//! #13 の「外部アクセスはすべて1か所を通す」を守らせているのは規約ではなく **crate の置き場所**
//! で、外部ツールの crate が `scheduler/` の子である限り、迂回はコンパイルエラーになる。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use escrow_tests::{Member, members};

/// Composition Root（#82）。`app/crates/` の中を組み立てる。
const APP: &str = "app";

/// `escrow-app` が組み立てる crate の置き場所。この下からの相対パスが役割の語。
const CRATES: &str = "app/crates";

/// 段1 のカーネル。`app/` の中では誰でも依存に持ってよい。`app/crates/` からの相対。
const DOMAIN: &str = "domain";

/// 段4 のスライスの群。`app/crates/` からの相対。
const SLICES: &str = "slices";

/// 入口。`escrow-app` だけを見る。**緩めて `app/crates/` を見せない** — 緩めた瞬間に、
/// 入口ごとにイベントストアを開いて組み立て直す経路ができ、画面ごとに足す関数が1つの入口にしか
/// 届かなくなる。
const UI: &str = "ui";

/// ワークスペース全体の守り。読むのは `Cargo.toml` とソースと規約だけ。
const TESTS: &str = "tests";

/// 外部ツールを呼ぶ crate と、その場所。
///
/// 包含の規則は「在るなら守る」しか言えず、`app/crates/` 直下へ動かすと「external を知るのは
/// scheduler だけ」（#13）が黙って消える。crate 名で書くのは、改名したときに次のテストが
/// 黙って通るのを防ぐため。
const EXTERNAL: (&str, &str) = ("escrow-external", "app/crates/scheduler/external");

/// `app/crates/` に横並びで置く crate の間で、左が右を依存に持ってよいもの。
///
/// 横並び同士の可否は構造から決まらないので、この分だけ表で持つ。パスは `app/crates/` からの
/// 相対で、スライスは群の `slices` にまとめる。`domain` は誰でも持ってよいので書かない。
/// **緩めて「横並びは全部よい」にしない** — スライスが `config` を読む経路ができる。設定の値は
/// `escrow-app` と `scheduler` が読んでスライスへ渡す。
const SIDE_BY_SIDE: &[(&str, &[&str])] = &[
    ("event-store", &[]),
    ("config", &[]),
    ("scheduler", &["config", "scheduler/external"]),
    ("scheduler/external", &["config"]),
    // handover は scheduler 抜きで足りる（#15）。表が言うのは持ってよいものの上限。
    ("slices", &["event-store", "scheduler"]),
];

/// 木の中の置き場所。依存してよいものはここから出る。
enum Place<'a> {
    App,
    /// `app/crates/` からの相対パス。
    Crate(&'a Path),
    Ui,
    Tests,
}

fn place(dir: &Path) -> Option<Place<'_>> {
    if dir == Path::new(APP) {
        Some(Place::App)
    } else if let Ok(role) = dir.strip_prefix(CRATES) {
        Some(Place::Crate(role))
    } else if dir.starts_with(UI) {
        Some(Place::Ui)
    } else if dir == Path::new(TESTS) {
        Some(Place::Tests)
    } else {
        None
    }
}

/// [`SIDE_BY_SIDE`] の鍵。スライスは1つ1つではなく群で引く。
fn row_key(role: &Path) -> &Path {
    if role.starts_with(SLICES) {
        Path::new(SLICES)
    } else {
        role
    }
}

/// `from` が `to` を依存に持ってはいけない理由。持ってよければ `None`。
///
/// `dirs` はワークスペースの全 member の置き場所。子（親の member の直下に在る member）を
/// 見分けるのに使う。
fn violation(from: &Member, to: &Member, dirs: &BTreeSet<&Path>) -> Option<&'static str> {
    let (Some(from_place), Some(to_place)) = (place(&from.dir), place(&to.dir)) else {
        return Some("木に無い場所に在る");
    };

    if to
        .dir
        .parent()
        .is_some_and(|parent| dirs.contains(parent) && parent != from.dir)
    {
        return Some("子を依存に持てるのは親だけ");
    }

    match (from_place, to_place) {
        (Place::Ui, Place::App) => None,
        (Place::Ui, _) => Some("入口が依存に持てるのは escrow-app だけ（#82）"),
        (Place::Tests, _) => Some("tests/ が読むのは Cargo.toml とソースと規約だけ"),
        (Place::App, Place::Crate(_)) => None,
        (Place::App, _) => Some("escrow-app が組み立てるのは app/crates/ の中"),
        (Place::Crate(_), Place::Crate(role)) if role == Path::new(DOMAIN) => None,
        (Place::Crate(from_role), Place::Crate(to_role))
            if from_role.starts_with(SLICES) && to_role.starts_with(SLICES) =>
        {
            Some("スライスは互いを知らない（#15）")
        }
        (Place::Crate(from_role), Place::Crate(to_role)) => {
            let from_key = row_key(from_role);
            let permitted = SIDE_BY_SIDE
                .iter()
                .find(|(key, _)| Path::new(key) == from_key)
                .is_some_and(|(_, deps)| deps.iter().any(|dep| Path::new(dep) == to_role));
            (!permitted).then_some("横並びの表（SIDE_BY_SIDE）に無い")
        }
        (Place::Crate(_), _) => Some("app/crates/ の中から見えるのは app/crates/ の中だけ"),
    }
}

/// 依存がすべて置き場所の規則に従っていること。
#[test]
fn dependencies_follow_the_tree() {
    let members = members();
    let by_name: BTreeMap<&str, &Member> = members.iter().map(|m| (m.name.as_str(), m)).collect();
    let dirs: BTreeSet<&Path> = members.iter().map(|m| m.dir.as_path()).collect();

    let mut violations = Vec::new();
    for from in &members {
        for dep in from.escrow_dependencies() {
            let to = by_name
                .get(dep.as_str())
                .unwrap_or_else(|| panic!("{dep} がワークスペースに無い"));
            if let Some(why) = violation(from, to, &dirs) {
                violations.push(format!(
                    "{} ({}) → {} ({}): {why}",
                    from.name,
                    from.dir.display(),
                    to.name,
                    to.dir.display()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "置き場所の規則に無い依存:\n{}",
        violations.join("\n")
    );
}

/// 外部ツールの crate が `scheduler/` の子であること（#13）。
#[test]
fn the_external_tools_stay_under_the_scheduler() {
    let (name, dir) = EXTERNAL;
    let members = members();
    let external = members
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("{name} がワークスペースに無い。改名したなら EXTERNAL も直す"));

    assert_eq!(
        external.dir,
        Path::new(dir),
        "{name} の置き場所。scheduler の子でなくなると、知ってよいのが scheduler だけという規則が消える"
    );
}

/// 全 member が木のどこかに在ること。
///
/// 木の外に在る member は [`violation`] が「木に無い場所」として落とすが、それは依存を持つ
/// member だけ。依存を持たないうちに置き場所を決めさせる。
#[test]
fn every_member_has_a_place_in_the_tree() {
    let outside: Vec<String> = members()
        .iter()
        .filter(|m| place(&m.dir).is_none())
        .map(|m| format!("{} ({})", m.name, m.dir.display()))
        .collect();

    assert!(
        outside.is_empty(),
        "木（{APP}/ · {CRATES}/ · {UI}/ · {TESTS}/）の外に在る member: {outside:?}"
    );
}

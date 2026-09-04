//! 共有カーネルが膨らまないこと（#15）。
//!
//! Vertical Slice の定番の失敗は、共有部分が育って元の `escrow-core` に戻ること。
//! 手は2つあり、片方は構造、もう片方は可視化。
//!
//! 1. **副作用を書けなくする** — 依存が3つしか無ければ、DB ヘルパも `async fn` も
//!    書けない。「入れない規律」ではなく「書けない状態」にする
//! 2. **モジュール一覧を固定する** — 副作用を持たない共有物は 1 では止まらない。
//!    足すには下の表を編集するしかなくなり、必ず差分に現れる

use std::collections::BTreeSet;
use std::path::PathBuf;

/// `escrow-domain` の `[dependencies]`。これ以外は入れない（#15）。
const DEPENDENCIES: &[&str] = &["chrono", "thiserror", "url"];

/// `[dev-dependencies]`。テストの中でだけ副作用を持てるなら、抜け道は在ることになる。
///
/// `toml` は下のテストが `Cargo.toml` を読むため、`tempfile` は `asset` の走査を
/// 実ディレクトリで試すため。どちらも公開 API に出ない。
const DEV_DEPENDENCIES: &[&str] = &["tempfile", "toml"];

/// `escrow-domain` に置いてよい公開モジュール。#1 の classDiagram と1対1（#15）。
///
/// **入場条件は「#1 に載っていること」。** 判断の根拠を外部に置くのが要点で、
/// 「これも共通だから」で入れ始めた瞬間に `escrow-core` が名前を変えて戻る。
const KERNEL: &[&str] = &[
    "asset",
    "content",
    "item",
    "liveness",
    "source",
    "state",
    "timestamp",
    "url",
];

/// 公開しない補助。#1 の語彙ではないので [`KERNEL`] には入れず、ここで別に許す。
const INTERNAL: &[&str] = &["id"];

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn manifest() -> toml::Table {
    let path = crate_root().join("Cargo.toml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .parse()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn table(manifest: &toml::Table, name: &str) -> BTreeSet<String> {
    manifest
        .get(name)
        .and_then(toml::Value::as_table)
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default()
}

fn expected(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

/// 依存はこの3つだけ。`sqlx` を足すと落ちる。
#[test]
fn the_kernel_cannot_reach_the_outside_world() {
    let manifest = manifest();

    assert_eq!(
        table(&manifest, "dependencies"),
        expected(DEPENDENCIES),
        "escrow-domain は同期・純関数だけを置く（#15）"
    );
    assert_eq!(
        table(&manifest, "dev-dependencies"),
        expected(DEV_DEPENDENCIES),
        "テストの中でだけ副作用を持てるなら、抜け道は在る"
    );
    assert_eq!(
        table(&manifest, "build-dependencies"),
        BTreeSet::new(),
        "ビルドスクリプトも外へは出ない"
    );
}

/// `src/` に在るファイルと、`lib.rs` が宣言しているモジュールが、
/// どちらも表と一致すること。
///
/// 両方を見るのは、宣言だけ、あるいはファイルだけを見ると片方が抜けるため。
/// 公開の別も見るので、`INTERNAL` のものを `pub mod` に格上げしても落ちる。
#[test]
fn only_the_listed_modules_exist() {
    let src = crate_root().join("src");

    let files: BTreeSet<String> = std::fs::read_dir(&src)
        .unwrap_or_else(|e| panic!("{}: {e}", src.display()))
        .map(|entry| entry.expect("読めるエントリ").file_name())
        .filter_map(|name| {
            let name = name.to_string_lossy().into_owned();
            name.strip_suffix(".rs").map(str::to_owned)
        })
        .filter(|name| name != "lib")
        .collect();

    let mut listed = expected(KERNEL);
    listed.extend(expected(INTERNAL));
    assert_eq!(files, listed, "表に無いモジュールが src/ に在る（#15）");

    let lib = std::fs::read_to_string(src.join("lib.rs")).expect("lib.rs");
    let declared = |prefix: &str| -> BTreeSet<String> {
        lib.lines()
            .filter_map(|line| line.strip_prefix(prefix))
            .filter_map(|rest| rest.strip_suffix(';'))
            .map(str::to_owned)
            .collect()
    };

    assert_eq!(declared("pub mod "), expected(KERNEL), "公開モジュールの表");
    assert_eq!(declared("mod "), expected(INTERNAL), "非公開モジュールの表");
}

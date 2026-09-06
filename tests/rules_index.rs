//! 規約ファイルが `CONTRIBUTING.md` の索引に載っていること（`docs/rules/amendment.md`）。
//!
//! 索引に載っていないファイルは、どの業務も読まない。規則を書いても届かないので、
//! ファイルを足した差分の中で落とす。

use escrow_tests::root;

/// 索引の表の行だけを取り出す。
///
/// 文書のどこかに名前が在ればよい形にすると、索引から落ちても検査の3層の表が同じ
/// 名前を持っているせいで通る。見出しが見つからなければ空になり、全ファイルが
/// 載っていない扱いで落ちる。
fn index_rows(contributing: &str) -> String {
    contributing
        .lines()
        .skip_while(|line| !line.starts_with("## 業務と、読むファイル"))
        .skip(1)
        .take_while(|line| !line.starts_with("## ") && !line.starts_with("---"))
        .filter(|line| line.starts_with('|'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_rule_file_is_in_the_index() {
    let root = root();
    let contributing = root.join("CONTRIBUTING.md");
    let rows = index_rows(
        &std::fs::read_to_string(&contributing)
            .unwrap_or_else(|e| panic!("{}: {e}", contributing.display())),
    );

    let rules = root.join("docs/rules");
    let mut missing: Vec<String> = std::fs::read_dir(&rules)
        .unwrap_or_else(|e| panic!("{}: {e}", rules.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("{}: {e}", rules.display()))
                .path()
        })
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .map(|path| {
            let name = path.file_name().expect("ファイル名").to_string_lossy();
            format!("docs/rules/{name}")
        })
        .filter(|relative| !rows.contains(relative.as_str()))
        .collect();
    missing.sort();

    assert!(
        missing.is_empty(),
        "規約ファイルが CONTRIBUTING.md の「業務と、読むファイル」の表に載っていない。\
         載っていないものはどの業務も読まないので、足した差分の中で索引へ載せる: {missing:?}"
    );
}

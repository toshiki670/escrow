//! 規約ファイルが `CONTRIBUTING.md` の索引に載っていること（`docs/rules/amendment.md`）。
//!
//! 索引に載っていないファイルは、どの業務も読まない。規則を書いても届かないので、
//! ファイルを足した差分の中で落とす。

use escrow_tests::root;

#[test]
fn every_rule_file_is_in_the_index() {
    let root = root();
    let index = std::fs::read_to_string(root.join("CONTRIBUTING.md")).expect("CONTRIBUTING.md");

    let mut missing: Vec<String> = std::fs::read_dir(root.join("docs/rules"))
        .expect("docs/rules/")
        .map(|entry| entry.expect("docs/rules/ の中身").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .map(|path| {
            let name = path.file_name().expect("ファイル名").to_string_lossy();
            format!("docs/rules/{name}")
        })
        .filter(|relative| !index.contains(relative.as_str()))
        .collect();
    missing.sort();

    assert!(
        missing.is_empty(),
        "規約ファイルが CONTRIBUTING.md の索引に載っていない。\
         載っていないものはどの業務も読まないので、足した差分の中で索引へ載せる: {missing:?}"
    );
}

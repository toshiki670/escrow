//! `macro_rules!` を禁じる（`CONTRIBUTING.md`）。

use escrow_tests::members;

#[test]
fn macro_rules_is_banned() {
    let declared: Vec<String> = members()
        .iter()
        .flat_map(|member| {
            member.sources().into_iter().flat_map(|(path, body)| {
                body.lines()
                    .map(str::trim_start)
                    .filter_map(|line| line.strip_prefix("macro_rules! "))
                    .filter_map(|rest| rest.split([' ', '{']).next())
                    .map(|name| format!("{}/{path}:{name}", member.name))
                    .collect::<Vec<_>>()
            })
        })
        .collect();

    assert!(
        declared.is_empty(),
        "macro_rules! は禁止。独立した crate に切り出すか、\
         ここの禁止を限定的な許可へ書き換える: {declared:?}"
    );
}

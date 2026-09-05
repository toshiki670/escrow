//! `macro_rules!` を禁じる。
//!
//! マクロは自由度が高いぶん、書き手の想定を外れた展開が起きてもコンパイラが助けない。
//! `derive`・generics・trait で書ける形を採る（`CONTRIBUTING.md`）。
//!
//! 本当に要るものが出てきたら、まず **escrow から独立した crate にできないか**を見る。
//! マクロで解くほど一般的な仕組みなら、escrow に閉じている理由が無いことが多い。それでも
//! 中に要るなら、この禁止を「限定的な許可」へ書き換える。何を許したかと理由がここに残る。

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

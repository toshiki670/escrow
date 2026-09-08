# レビュー

[escrow 規約](../../CONTRIBUTING.md) の一部。

**レビューは、成果物を出した業務とは別の業務が行う。**

## 渡すものと、回す契機

| 規則 | 出典 |
|---|---|
| **業務ごとに渡す。** 設計は Issue 本文を実装へ渡す前に、実装は PR をマージする前に、QA は守りを作ったら、レビューへ渡す。**始めるのは出した側。** 差分だけでは渡さない | escrow（招集の置き換えは下の行） |
| **author と reader を別の業務が担う。** 業務を兼ねること自体は禁じない | [Fagan inspection](https://en.wikipedia.org/wiki/Fagan_inspection)（役は Fagan, *Design and Code Inspections to Reduce Errors in Program Development*, 1976 と Eickelmann ら 2003。このページから採った）。author と reader は別の役で、reader が成果物を paraphrase する。**escrow に inspection の会議は無いので reader の役をレビューの業務へ当て、原典で招集する moderator の役を出した側へ当てた（escrow）** |

## 見る範囲

| 規則 | 出典 |
|---|---|
| **差分の外を読む。** 変更そのものだけを読んでも、影響は見えない | [Bacchelli & Bird, 2013](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/ICSE202013-codereview.pdf)「When reviewing a small, unfamiliar change, it is often necessary to read through much more code than that being reviewed」 |
| **出した側が示した範囲を、範囲の定めとして使わない。** 説明は読む助けとして使う | 同上「people can say they are doing one thing, while they are doing many more of them」「the description is not enough」 |
| 変更の理由が分かるまで読む。分からないままなら、それを指摘として出す | 同上「the most difficult thing when doing a code review is understanding the reason of the change」 |

## 回す数

| 規則 | 出典 |
|---|---|
| 既定は2周 — レビューと、直しを確かめる周 | [Fagan inspection](https://en.wikipedia.org/wiki/Fagan_inspection)（段階は Fagan, *Advances in Software Inspections*, 1986）。**Preparation と Inspection meeting が1周目、Follow-up が2周目に当たる。escrow のレビューは1回の読みで、材料を読むこと（Preparation）と欠陥を挙げること（Inspection meeting）を両方行うので2段を1周へまとめ、原典で Follow-up を確かめる moderator の役をレビューの業務へ当てた（escrow）。** 原典の Rework は author が直す段で、周に数えない |
| 3周目に入るのは、指摘を受けて**規則や決定の本文が変わったとき**だけ。字句や PR 本文だけの直しでは回さない | 同上の Follow-up 節「In non-trivial cases, a full re-inspection is performed by the inspection team (not only the moderator)」。**ページはこの一文に出典を付けていない。** 原典の "non-trivial" を「規則や決定の本文が変わったとき」と定め、再検査を行う inspection team の役をレビューの業務へ当てた（escrow） |
| **3周を越えたら、レビューを続けずに発注側へ返す。** 出すか、設計へ戻すかを決めるのは発注側 | escrow |

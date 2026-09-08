# レビュー

[escrow 規約](../../CONTRIBUTING.md) の一部。

**レビューは、成果物を出した業務とは別の業務が行う。**

## 渡すものと、回す契機

| 規則 | 出典 |
|---|---|
| 成果物ができたら、出した側がレビューへ渡す。設計は Issue 本文、実装は PR（差分と本文）、QA は守り。差分だけでは渡さない | escrow |
| **author は、自分が書いた成果物の reader にならない。** 業務を兼ねること自体は禁じない | [Fagan inspection](https://en.wikipedia.org/wiki/Fagan_inspection)（Fagan, *Design and Code Inspections to Reduce Errors in Program Development*, IBM Systems Journal, 1976）。author と reader は別の役で、reader が成果物を paraphrase する。**escrow に inspection の会議は無いので、reader の役をレビューの業務へ当てた（escrow）** |
| 招集は出した側が行う。**原典で招集するのは moderator だが、escrow にその役は無い（escrow）** | 同上 |

## 見る範囲

| 規則 | 出典 |
|---|---|
| **範囲は、出した側が示した範囲に閉じない。** 差分の外を読む | [Bacchelli & Bird, 2013](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/ICSE202013-codereview.pdf)「When reviewing a small, unfamiliar change, it is often necessary to read through much more code than that being reviewed」 |
| 出した側の説明は、読む助けとして使い、範囲の定めとして使わない | 同上「the description is not enough」「people can say they are doing one thing, while they are doing many more of them」 |
| 変更の理由が分かるまで読む。分からないままなら、それを指摘として出す | 同上「the most difficult thing when doing a code review is understanding the reason of the change」 |

## 回す数

| 規則 | 出典 |
|---|---|
| 既定は2周 — レビューと、直しを確かめる周 | 同上。Fagan の **Inspection meeting** が1周目、**Follow-up** が2周目に当たる。**原典の Rework は author が直す段で、周に数えない。inspection の会議が無いので、Inspection meeting をレビューの業務へ当てた（escrow）** |
| 3周目に入るのは、指摘を受けて**規則や決定の本文が変わったとき**だけ。字句や PR 本文だけの直しでは回さない | 同上「In non-trivial cases, a full re-inspection is performed by the inspection team (not only the moderator)」 |
| **3周を越えたら、レビューを続けずに発注側へ返す。** 出すか、設計へ戻すかを決めるのは発注側 | escrow |

# レビュー

[escrow 規約](../../CONTRIBUTING.md) の一部。

**レビューは、成果物を出した業務とは別の業務が行う。**

## 渡すものと、回す契機

| 規則 | 出典 |
|---|---|
| **業務ごとに渡す。** 設計は Issue 本文を実装へ渡す前に、実装は PR をマージする前に、QA は守りを作ったら、レビューへ渡す。**始めるのは出した側。** 差分だけでは渡さない | escrow（招集の置き換えは下の行） |
| **author と reader を別の業務が担う。** 業務を兼ねること自体は禁じない | [Fagan inspection](https://en.wikipedia.org/wiki/Fagan_inspection)（役は Fagan, *Design and Code Inspections to Reduce Errors in Program Development*, 1976 と Eickelmann ら 2003。このページから採った）。author と reader は別の役で、reader が成果物を paraphrase する。**escrow に inspection の会議は無いので reader の役をレビューの業務へ当て、原典で招集する moderator の役を出した側へ当てた（escrow）** |
| **渡したら、渡した成果物へコメントを1件残す。** 何周目かと、渡したものを書く —— 設計の成果物なら Issue へ、実装と QA の成果物なら PR へ | escrow |

## 見る範囲

| 規則 | 出典 |
|---|---|
| **差分の外を読む。** 変更そのものだけを読んでも、影響は見えない | [Bacchelli & Bird, 2013](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/ICSE202013-codereview.pdf)「When reviewing a small, unfamiliar change, it is often necessary to read through much more code than that being reviewed」 |
| **出した側が示した範囲を、範囲の定めとして使わない。** 説明は読む助けとして使う | 同上「people can say they are doing one thing, while they are doing many more of them」「the description is not enough」 |
| 変更の理由が分かるまで読む。分からないままなら、それを指摘として出す | 同上「the most difficult thing when doing a code review is understanding the reason of the change」 |

## 回す数

| 規則 | 出典 |
|---|---|
| **周は成果物ごとに数える。** 進むのは、その周で**渡した**成果物の側（上の「渡すものと、回す契機」が定める、**出した側が渡す**行為）。**読んだだけの成果物の数は進まない。** 通算では数えない。**満たすべきものが変わっても数え直さない** | [Fagan inspection](https://en.wikipedia.org/wiki/Fagan_inspection)（Criteria と Typical operations は Fagan, *Advances in Software Inspections*, 1986）「The exit criteria are specified in a high-level document, which is then used as the standard to which the operation result (**low-level document**) is compared during the inspection」「the **low-level document** is corrected until the requirements in the high-level document are met」「If verification fails, go back to the rework process」（**Follow-up 節にページは出典を付けていない**）。**原文の low-level document を escrow の成果物へ、high-level document をその成果物が満たすべきもの（Issue 本文なら発注側の指示と規約、PR なら Issue 本文と規約、守りなら規約）へ当てた（escrow）** |
| **成果物が次の業務へ渡ったら、数え直す。** 渡る先は上の「渡すものと、回す契機」が定める —— 設計の成果物は実装へ渡したとき、実装と QA の成果物はマージしたとき | [Fagan inspection](https://en.wikipedia.org/wiki/Fagan_inspection)「**Entry criteria** are the criteria or requirements which must be met to **enter a specific process**」「**Exit criteria** are the criteria or requirements which must be met to **complete a specific process**」。**1つの inspection は entry から exit までの1回。escrow の周をその1回の中の数と読み、次の業務へ渡ることを exit へ当てた（escrow）** |
| **設計の成果物の既定は1周。** レビューだけで、**直しを確かめる周を置かない** | [Fagan inspection](https://en.wikipedia.org/wiki/Fagan_inspection)（段階は Fagan, *Advances in Software Inspections*, 1986）。下の行と同じく Preparation と Inspection meeting を1周へまとめ、**そのうえで設計の成果物には Follow-up を当てない。設計の rework は exit criteria の検査を受けないまま実装へ出る（escrow）** |
| **実装と QA の成果物の既定は2周** — レビューと、直しを確かめる周 | [Fagan inspection](https://en.wikipedia.org/wiki/Fagan_inspection)（段階は Fagan, *Advances in Software Inspections*, 1986）。**Preparation と Inspection meeting が1周目、Follow-up が2周目に当たる。escrow のレビューは1回の読みで、材料を読むこと（Preparation）と欠陥を挙げること（Inspection meeting）を両方行うので2段を1周へまとめ、原典で Follow-up を確かめる moderator の役をレビューの業務へ当てた（escrow）。** 原典の Rework は author が直す段で、周に数えない |
| 3周目に入るのは、指摘を受けて**規則や決定の本文が変わったとき**だけ。字句や PR 本文だけの直しでは回さない | 同上の Follow-up 節「In non-trivial cases, a full re-inspection is performed by the inspection team (not only the moderator)」。**ページはこの一文に出典を付けていない。** 原典の "non-trivial" を「規則や決定の本文が変わったとき」と定め、再検査を行う inspection team の役をレビューの業務へ当てた（escrow） |
| **3周を越えたら、レビューを続けずに発注側へ返す。** 出すか、設計へ戻すかを決めるのは発注側 | escrow |

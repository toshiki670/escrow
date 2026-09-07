# 設計

[escrow 規約](../../CONTRIBUTING.md) の一部。

## Issue の切り方

**Issue は Vertical Slice で切る。** 1つの Issue が閉じたら、利用者視点で評価できる価値が
1つ増える。

| 規則 | 出典 |
|---|---|
| 層で切らない。水平分割は1つの層だけを変えるので、**他の層と組み合わせるまで利用者に見える価値がない** | [Vertical / Horizontal Slicing](https://www.visual-paradigm.com/scrum/user-story-splitting-vertical-slice-vs-horizontal-slice/)。「[アーキテクチャ](architecture.md)」の Vertical Slice を Issue の分け方へ当てたもの |
| 最初の1本は端から端まで細く通す。最終のアーキテクチャでなくてよいが、主要な部品を繋ぐ | Walking Skeleton（[Cockburn, *Crystal Clear*, 2004](https://books.google.com/books/about/Crystal_Clear.html?id=O_cMM5ztyMIC)） |
| **アーキテクチャは1本目で決まらない。** アーキテクチャを組み直す差分は、利用者に見える価値を1つ増やす Issue の中に置く。アーキテクチャだけを直す Issue を立てない | Incremental Rearchitecture（同上）。Walking Skeleton の対で、開発を止めて直すのではなく、動かしたまま段階的に組み直す |
| 1つの Issue が別々の理由で書き換わるようになったら分ける。分けるかどうかは、設計中に内容を見て判断する | [Divergent Change](https://refactoring.guru/refactoring/smells)（Fowler, *Refactoring*） |

**アーキテクチャの行は、機能を作る Issue に掛かる。** 依存 crate の破壊的変更への追随・toolchain の更新・
規約の改修そのものは、利用者に見える価値を増やさなくてよい。

### INVEST

**Issue が切れているかは6つで見る**（[INVEST](https://en.wikipedia.org/wiki/INVEST_(mnemonic))、Wake, 2003）。
原則そのものは出典を見る。ここは escrow での読み方だけを置く索引。

| 文字 | escrow での読み方 |
|---|---|
| **I**（Independent） | 受け入れを、閉じた Issue の成果だけで確かめられる。確かめられないものは「前提」へ書き出す |
| **N**（Negotiable） | 本文は契約ではなく、発注側と受注側が詰める場。詰め方は下の「起票と合意」 |
| **V**（Valuable） | 完成したものが、実際の利用者の必要を満たす |
| **E**（Estimable） | 着手する前に、何を作れば閉じるかが本文から読める |
| **S**（Small） | 「作るもの」は、「受け入れ」を成立させる分に収まる |
| **T**（Testable） | 本文だけで、閉じた判定ができる。「受け入れ」を持つ |

## 起票と合意

**Issue は合意形成の場。** その Issue で何を作るかを決める側を**発注側**、それに答えて作る側を
**受注側**と呼ぶ。

問いの内容を決めるのは発注側で、受注側が出すのは Position（案）と Argument（賛否）
（[IBIS](https://escholarship.org/uc/item/5cj786v8)、Kunz & Rittel, 1970）。**分かれ目は合意の
有無。** 発注側の合意があれば、書き換えの操作は受注側が行ってよい。

### 起票は指示の範囲と一致させる

1. 指示の語をそのまま拾って対象を書き出す。指示に無い語を対象へ足さない
2. 題は指示の語で付ける
3. 隣接する話題を足したくなったら、足さずに1行で訊く

### 要件を足すときは、書き換える前に合意を取る

1. 足したい要件を Position として出す。この時点で本文を書き換えない
2. Argument を添える — なぜ既存の要件では足りないか
3. **発注側の理解と合意を得てから書き換える**
4. 書き換えたら、何を足したかを発注側へ見せる

## 決定の書き方

| 規則 | 出典 |
|---|---|
| 決定には、それが**揺らぐ条件**を添える。理由だけでは、次の人が見直す契機を持てない | [Toulmin model](https://www.humanities.mcmaster.ca/~hitchckd/Toulminswarrants.pdf) の rebuttal / qualifier（Toulmin, *The Uses of Argument*, 1958） |

理由そのものを添えることは「[記録の置き場所](records.md)」が要求している。

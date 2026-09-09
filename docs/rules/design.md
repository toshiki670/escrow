# 設計

[escrow 規約](../../CONTRIBUTING.md) の一部。

## Issue の切り方

**Issue は Vertical Slice で切る。** 1つの Issue が閉じたら、customer が評価できる価値が
1つ増える。

| 規則 | 出典 |
|---|---|
| 層で切らない。水平分割は1つの層だけを変えるので、**他の層と組み合わせるまで customer にとってほとんど価値がない** | [INVEST](https://xp123.com/articles/invest-in-good-stories-and-smart-tasks/) の V（Wake, 2003）「a full database layer (for example) has little value to the customer if there's no presentation layer」。**「[アーキテクチャ](architecture.md)」の Vertical Slice を Issue の分け方へ当てたもの（escrow）** |
| 最初の1本は端から端まで細く通す。最終のアーキテクチャでなくてよいが、主要な部品を繋ぐ | [Walking Skeleton](https://web.archive.org/web/20170214035145/http://alistair.cockburn.us/Walking+skeleton)（Cockburn, *Crystal Clear*, 2004） |
| **アーキテクチャの組み直しで、価値が増えるのを止めない。** 組み直しは段階に分け、その間も customer から見た価値が増え続ける | [Incremental Rearchitecture](https://web.archive.org/web/20170629015916/http://alistair.cockburn.us/Incremental+Rearchitecture)（Cockburn, *Crystal Clear*, 2004）。Walking Skeleton の対。原文の「開発を止めない」を Vertical Slice の語彙へ当てたもの（escrow） |
| 1つの Issue が別々の理由で書き換わるようになったら分ける。分けるかどうかは、設計中に内容を見て判断する | [Divergent Change](https://refactoring.guru/smells/divergent-change)（refactoring.guru）「Divergent Change is when many changes are made to a single **class**」。**原文の単位は class。Issue の分け方へ当てたもの（escrow）** |

### 価値の宛先

**価値の宛先は customer。** その Issue が変えるものに対して定まる。

| 規則 | 出典 |
|---|---|
| 宛先は customer。誰でもよい相手ではない | [INVEST](https://xp123.com/articles/invest-in-good-stories-and-smart-tasks/) の V（Wake, 2003）「We don't care about value to just anybody; it needs to be valuable to the customer」 |
| 開発者の関心も、customer が重要だと認める形に組み立てれば価値になる | [INVEST](https://xp123.com/articles/invest-in-good-stories-and-smart-tasks/) の V（Wake, 2003）「Developers may have (legitimate) concerns, but these framed in a way that makes the customer perceive them as important」 |
| 宛先は、その Issue が変えるものに対して定まる。変えるものの単位は escrow か規約で、部品を単位にしない。**escrow を変える Issue の customer は escrow を使う人、規約を変える Issue の customer は規約を引く4業務の実施者** | escrow |

**規約を変える Issue では、customer と書き手が同じ人になる。** そのとき「開発者の関心も…」の行は縛りにならないので、**層だけを作る Issue を止めるのは「部品を単位にしない」の行と、上の「層で切らない」**（escrow）。

### 目標の高さ

**1つの Issue は user-goal の高さに置く。** 下位 Issue を束ねるものは strategic、user-goal を果たすために要るものは subfunction に置く。

| 段（原文の単位は use case） | escrow での読み方 |
|---|---|
| strategic（"white"） | 下位 Issue の目次として書く。受け入れは「下位 Issue が全部閉じている」 |
| **user-goal（"blue"）** | customer（原文は primary actor）が1つの仕事を片づけ、そこで手を止められる高さ |
| subfunction（"indigo" / "black"） | 要るときだけ書く。**"black" は、これ以上展開せず上位の Issue へ畳む印** |

出典は [Goal Levels](https://people.inf.elte.hu/molnarba/Informaciorendszerek_ELTE/Writing_effective_Use_cases_Cockburn.pdf)（Cockburn, *Writing Effective Use Cases*、1999年の草稿 p.46-52。刊行は 2001年）。

**段は語ではなく、片づく仕事で決まる**（同上 p.51「Every sentence will be written as a goal, and every goal could be unfolded into its own use case. We cannot tell by looking at the writing which sentences have been unfolded into separate use cases, and which have not」、p.50「Both are blue goals at different times」）。原文が indigo の例に挙げる "Find a product" / "Find a Customer" のような形でも、**escrow の customer がそれで1つの仕事を片づけるなら "blue" に置く（escrow）。**

### Issue 本文の節

**中身が無い節は置かない。**「前提」と「受け入れ」は必ず置き、「前提」が無ければ「無し」と書く。

| 節 | 中身 |
|---|---|
| 前提 | 着手する前に閉じている Issue |
| 作るもの | この Issue で作るもの |
| 決めること | 閉じるまでに決める未決 |
| 受け入れ | 閉じた判定。目視か実行で確かめられる形で書く |
| 関連 | 経緯が読める Issue |

### INVEST

**Issue が切れているかは6つで見る**（[INVEST](https://xp123.com/articles/invest-in-good-stories-and-smart-tasks/)、Wake, 2003）。
原則そのものは出典を見る。ここは escrow での読み方だけを置く索引。

| 文字 | escrow での読み方 |
|---|---|
| **I**（Independent） | 「受け入れ」の確認に要る Issue が、上の「前提」に在る |
| **N**（Negotiable） | 下の「起票と合意」が果たす |
| **V**（Valuable） | 上の「価値の宛先」が果たす |
| **E**（Estimable） | 「作るもの」を書き切れる。書けない部分は「決めること」へ出す |
| **S**（Small） | **いま「受け入れ」を確かめる分だけ作る。** 最終の形まで作り込むなら、要らない分を次の Issue へ送る。作り込みの深さを見る物差しで、目標の高さは上の「目標の高さ」。**原文の Small は "Stories typically represent at most a few person-weeks worth of work" という作業量の物差しだが、escrow は見積り時間のような外部の尺度を使わないので、いま確かめる範囲へ置き換えた（escrow）** |
| **T**（Testable） | 上の「受け入れ」が果たす |

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

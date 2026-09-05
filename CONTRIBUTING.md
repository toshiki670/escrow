# escrow 規約

**規則には出典を置き、守れているかは機械が確かめる。** 出典を持たない規則は、escrow の
判断であることを明示する。

境界は1つ。**何を作るかは Issue が決め、どう作るかをここが決める。**

---

## 決定の置き場所

**Architecture Decision Record**（[Nygard, 2011](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)）。
決定・文脈・帰結を1か所に残す。escrow ではそれが Issue の本文にあたる — #1 データ構造、
#2 設定、#3 配布、#4 外部インターフェース、#5 取得経路、#6 画面、#13 スケジューラ、
#15 アーキテクチャ、#7 実装の順序。

> A new person coming on to a project may be perplexed, baffled, delighted, or infuriated
> by some past decision. Without understanding the rationale or consequences, this person
> has only two choices: blindly accept the decision or blindly change it.

**コードは Issue 番号で参照し、本文を写さない。**
[**DRY**](https://pragprog.com/tips/)（Hunt & Thomas, *The Pragmatic Programmer*, Tip 15）—
「Every piece of knowledge must have a single, unambiguous, authoritative representation
within a system.」写しを作ると、Issue を直してもコードが古いまま残る経路ができる。

この規約も同じ制約を受ける。ここに置くのは**作り方の規則だけ**にする。

---

## ドキュメント

### 形

[**Rust RFC 1574**](https://rust-lang.github.io/rfcs/1574-more-api-documentation-conventions.html)
（[RFC 505](https://rust-lang.github.io/rfcs/0505-api-comment-conventions.html) を改訂）に従う。

| 規則 |
|---|
| 要約は1行。三人称・現在形（「Return」ではなく「Returns」） |
| 見出しは `Examples` / `Panics` / `Errors` / `Safety`。1つしか無くても複数形 |

### 中身

[**Rust API Guidelines**](https://rust-lang.github.io/api-guidelines/documentation.html) に従う。

| ID | 規則 |
|---|---|
| C-FAILURE | 失敗・パニック・安全性を書く |
| C-LINK | 関連するものへリンクを張る |
| C-HIDDEN | 実装の詳細を rustdoc に出さない |

### 何を書くか

**Code Complete**（McConnell, 2004 — 32章 *Self-Documenting Code*）。

> Good code is its own best documentation. As you're about to add a comment, ask yourself,
> "How can I improve the code so that this comment isn't needed?"

コメントが持つのは **why**、コードが持つのが **how**。**補足が増えたら、まず設計を疑う** —
置き場所・構造・名前で解けるものを日本語で補っている状態は、質が落ちている合図。

```rust
// 良い — その形にした理由
/// 投影のスキーマを変えたいときは移行ではなくこれを走らせる。移行に混ぜると、
/// いつの間にか投影に手作業の移行が当たって作り直せなくなる。

// 悪い — コードが言っていることの繰り返し
/// item テーブルを DROP して、CREATE して、ログから INSERT する。
```

**次にここを触る人が間違えることだけ残す。** レビューや相談の場で説明のために作った比較・
言い換え・経緯は、そのときの相手に向けたもの。

```rust
// 悪い — 説明のために作った比較が残っている
/// 識別子の newtype とはそこが違う — あちらは包むのが仕事だが、
/// こちらは作る道を絞るのが仕事。

// 良い
/// 導出は `Display` まで。`Constructor` と `From` はこの2つの道を迂回させるので外してある。
```

### 言葉

| 規則 | 出典 |
|---|---|
| 指しているものの名前で書く。比喩で名を付けない | **Ubiquitous Language**（Evans, *Domain-Driven Design*, 2003）— 設計の語彙とコードの語彙を1つにする |
| 能動態で書く | [Google developer documentation style guide — Active voice](https://developers.google.com/style/voice) |
| 肯定形で書く。「作れない」ではなく「作る道は2つ」 | 同上（能動態の系） |
| 要求のレベルで書く。「`sqlx` がこう返すから」ではなく「#1 が `NULL` を許す列をこう決めているから」 | escrow の判断 |
| 推測は結論の隣に置く | escrow の判断 |
| 型で締めなかった所は、そう書く | #7 の決め事 —「決めて緩めた」と「締め忘れた」を区別する |

---

## 実装

### 一番弱い道具で書く

[**Rule of Least Power**](https://www.w3.org/2001/tag/doc/leastPower.html)（Berners-Lee &
Mendelsohn, W3C TAG, 2006）— 目的に足りる範囲で、最も弱い道具を選ぶ。弱い道具ほど、
外から中身を読み解ける。

**`macro_rules!` はワークスペース全体で禁じている。** `derive`・generics・trait で書ける形を
採る。本当に要るものが出てきたときの道は2つ。

1. **escrow から独立した crate にする。** マクロで解くほど一般的な仕組みなら、escrow に
   閉じている理由が無いことが多い
2. **禁止を「限定的な許可」へ書き換える。** 何を許したかと理由が `tests/macros.rs` に残る

### 依存を採るかどうか

[**Our Software Dependency Problem**](https://research.swtch.com/deps)（Cox, 2019）の見方に
従い、そのうえで escrow の2つを足す。順に見て、どれかで止まったら見送る。

| # | 見るもの | 出典 |
|---|---|---|
| 1 | issue tracker — 未解決の数と、放置されている期間 | Cox |
| 2 | commit 履歴 — いま手が入っているか、書き手が何人か | Cox |
| 3 | 推移的な依存 — 間接のものも自分の欠陥になる | Cox |
| 4 | 置き換える対象に、テストすべき振る舞いがあるか（分岐・変換・境界・単位） | escrow |
| 5 | 呼ぶ側の負担が変わらないか（綴りの変更・型注釈・`.to_owned()` はどれも増加） | escrow |

4 は Cox の費用便益を escrow へ当てたもの。`Self(x.into())` と `&self.0` のように振る舞いが
無いものは、採っても行数が減るだけで、依存の費用だけが残る。

### 標準の綴りに寄せる

公式のガイドラインが**どちらかを優位と書いていなければ、揃っているほうを崩さない**。
`new` と `From` は
[C-CTOR](https://rust-lang.github.io/api-guidelines/predictability.html) と
[C-CONV-TRAITS](https://rust-lang.github.io/api-guidelines/interoperability.html) の
どちらも相手を否定しておらず、std 自身が `String::new` と `String::from` の両方を持つ。

`as_str` は std 自身の綴り（`String::as_str`）で、`AsRef` は generic の境界に使う別の道具。
エディションや言語機能の更新で簡潔に書けるようになったものは、取り込む。

### 同じ仕事は同じ形

[**Rust API Guidelines — Predictability**](https://rust-lang.github.io/api-guidelines/predictability.html)。
1つの仕事に複数の書き方が混ざったら、どれかへ寄せる。

| 仕事 | 形 |
|---|---|
| 値を包む newtype を作る | `X::new(...)` |
| 内側の値を取り出す | `as_str()`（借用）/ `i64::from(x)`（`Copy`） |

見送った導出は、理由をその型の doc に1行残す。上の Nygard の引用がそのまま当てはまる —
理由が残っていなければ、次の人は盲目的に受け入れるか、盲目的に変えるかしかできない。

---

## 守り

[**Poka-yoke**](https://en.wikipedia.org/wiki/Poka-yoke)（新郷重夫, トヨタ生産方式）—
間違いが起きない形に作り替える。**この規約に「Xをするな」と書くだけなら訓練であり、訓練は
劣化する。** 守らせたい約束は、破ったときに落ちる形へ持っていく。

**ワークスペース全体にかかるものは `tests/` に置く。** member の一覧を `Cargo.toml` から
読むので、crate をどこへ置いても届く。1つの crate に閉じたものは、その crate の `tests/` へ。

| 守るもの | 落ちる場所 |
|---|---|
| `macro_rules!` の禁止 | `tests/macros.rs` |
| crate の依存の向き、スライス同士の隔離 | `tests/dependency_direction.rs` |
| カーネルの依存とモジュールの一覧 | `crates/escrow-domain/tests/kernel.rs` |
| `escrow-ledger` の公開 API、投影へ書く SQL の置き場所 | `crates/escrow-ledger/tests/public_api.rs` |
| 状態遷移の網羅（9状態 × 11事象のうち 20 本） | `crates/escrow-domain/src/state.rs` |
| 事象の順序と競合 | `item_event(item_id, seq)` の UNIQUE |

---

## 動かす

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo test --workspace --doc      # nextest は doctest を実行しない
```

SQL を変えたら `.sqlx/` を取り直す。手順は `.cargo/config.toml` の先頭にある。

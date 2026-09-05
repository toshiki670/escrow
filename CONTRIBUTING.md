# escrow 規約

**実装者とレビュアの判断基準。** 何かを書く前と、レビューで判断が割れたときに引く。

**機械が落とすのは「[守り](#守り)」の表にあるものだけ。** それ以外はレビューで見る。
判断が割れたら出典に当たる。出典を持たない規則は、escrow の判断であることを明示する。

---

## 決定の置き場所

**Issue は作業の単位で、実装が終われば閉じる。** 置き場所は4つあり、**何が残り続けるか**で
決まる。

| 置き場所 | 残すもの |
|---|---|
| この規約 | 閉じたあとも効き続ける規則 |
| Issue | 設計の決定と、そこに至る経緯。閉じても読める |
| PR | **その変更で何をどう判断したか。** 差分に紐づくので、後から差分だけを見た人が理由に辿れる |
| コードの doc | 次にそこを触る人が間違えること |

escrow は
[**Architecture Decision Record**](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
（Nygard, 2011）をファイルで持たず、閉じた Issue がその役をする。コードからは番号で参照する。
理由が残っていない決定は、次の人が盲目的に受け入れるか盲目的に変えるかしかできなくなる。

**本文を写さない。** [**DRY**](https://pragprog.com/tips/)（Hunt & Thomas,
*The Pragmatic Programmer*, Tip 15）—「Every piece of knowledge must have a single,
unambiguous, authoritative representation within a system.」写しを作ると、片方を直しても
もう片方が古いまま残る。この規約自身も同じ制約を受ける。

---

## アーキテクチャ

| 軸 | 採るもの | 出典 |
|---|---|---|
| コードの分け方 | **Vertical Slice** — 技術ではなくライフサイクルの段階で切る | [Bogard, 2018](https://www.jimmybogard.com/vertical-slice-architecture/)「Minimize coupling between slices, and maximize coupling in a slice.」 |
| 状態の持ち方 | **Event Sourcing** — 状態ではなく、状態を変えた事象を保存する | [Fowler, 2005](https://martinfowler.com/eaaDev/EventSourcing.html)「Capture all changes to an application state as a sequence of events.」 |
| 読み書きの分け方 | **CQRS** — 書くのは事象、読むのは投影 | [Young / Fowler, 2011](https://martinfowler.com/bliki/CQRS.html)「you can use a different model to update information than the model you use to read information」 |

```
段5  escrow-cli · escrow-gui
段4  discovery · acquisition · transcription · custody · handover
段3  escrow-scheduler
段2  escrow-ledger · escrow-external · escrow-config
段1  escrow-domain
```

**下の段しか見えない。** 段4 のスライスは、加えて**互いを知らない** — 順序はコードの
呼び出し順ではなく状態機械が持ち、スライスは投影を状態で絞って拾われる。段2 の中には
`escrow-external` → `escrow-config` の**1本だけ**が在る。

| 段 | 依存してよいもの |
|---|---|
| 1 `escrow-domain` | 外部 crate のみ。同期・純関数だけを置く |
| 2 `escrow-config` | なし |
| 2 `escrow-ledger` | domain |
| 2 `escrow-external` | domain / config |
| 3 `escrow-scheduler` | domain / config / external。**外部アクセスの port はこの crate の公開 API** |
| 4 スライス | domain / ledger / scheduler。`handover` は外へ出ないので scheduler も要らない |
| 5 入口 | 段1〜4。external だけは名前で知らない |

書く道は `Ledger::discover` と `Ledger::append` の2つだけで、投影は捨てて作り直せる
（`Ledger::rebuild`）。

---

## ドキュメント

### 形

**doc は日本語で書く。**
[**Rust RFC 1574**](https://rust-lang.github.io/rfcs/1574-more-api-documentation-conventions.html)
（[RFC 505](https://rust-lang.github.io/rfcs/0505-api-comment-conventions.html) を改訂）の
うち、言語に依らない部分に従う。

| 規則 | 出典 |
|---|---|
| 要約は1行。rustdoc の一覧に出るのはここだけ | RFC 1574 |
| 言い切りで書く（英語の三人称・現在形にあたるもの） | RFC 1574 を日本語へ読み替え |
| `Examples` / `Panics` / `Errors` / `Safety` の節を**書くときは**この名前を使い、1つしか無くても複数形にする。それ以外の見出しは日本語で自由に付けてよい | RFC 1574 |

### 中身

[**Rust API Guidelines**](https://rust-lang.github.io/api-guidelines/documentation.html)。

| ID | 規則 |
|---|---|
| C-FAILURE | 失敗・パニック・安全性を書く |
| C-LINK | 関連するものへリンクを張る |
| C-HIDDEN | 実装の詳細を rustdoc に出さない |

### 何を書くか

**Code Complete**（McConnell, 2004 — 32章 *Self-Documenting Code*）—「As you're about to
add a comment, ask yourself, 'How can I improve the code so that this comment isn't needed?'」

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
| 能動態で書く | [Google developer documentation style guide](https://developers.google.com/style/voice) |
| 肯定形で書く。「作れない」ではなく「作る道は2つ」 | 同上（能動態の系） |
| 要求のレベルで書く。「`sqlx` がこう返すから」ではなく「#1 が `NULL` を許す列をこう決めているから」 | escrow の判断 |
| 推測は結論の隣に置く | escrow の判断 |
| 型で締めなかった所は、そう書く | #7 の決め事 —「決めて緩めた」と「締め忘れた」を区別する |

---

## 実装

### 一番弱い道具で書く

[**Rule of Least Power**](https://www.w3.org/2001/tag/doc/leastPower.html)（Berners-Lee &
Mendelsohn, W3C TAG, 2006）— 目的に足りる範囲で、最も弱い道具を選ぶ。弱い道具ほど、外から
中身を読み解ける。

**`macro_rules!` はワークスペース全体で禁じている。** `derive`・generics・trait で書ける形を
採る。本当に要るものが出てきたときの道は2つ。

1. **escrow から独立した crate にする。** マクロで解くほど一般的な仕組みなら、escrow に
   閉じている理由が無いことが多い
2. **禁止を「限定的な許可」へ書き換える。** 何を許したかと理由が `tests/macros.rs` に残る

### 依存を採るかどうか

**見送る条件。** どれか1つでも当たれば採らない。

| 条件 | 出典 |
|---|---|
| 置き換える対象に、テストすべき振る舞いが無い — 分岐・変換・境界・単位のどれも無い。`Self(x.into())` と `&self.0` がこれで、採っても行数が減るだけ | escrow の判断 |
| 呼ぶ側の負担が増える — 綴りの変更・型注釈が要るようになる・`.to_owned()` が要る | escrow の判断 |
| **入れる目的が、いまある API の綴りを変えることだけ**で、かつ公式のガイドラインがいまの綴りを否定していない — 揃っているものを崩すだけになる | escrow の判断 |

**調べて PR に残すこと。** **閾値は置かない** — 同じ数字でも crate によって意味が違うので、
見た結果と、そこから何を判断したかを書く。

| 見るもの | 出典 |
|---|---|
| issue tracker — 未解決の数と、放置されている期間 | [Cox, 2019](https://research.swtch.com/deps) |
| commit 履歴 — いま手が入っているか、書き手が何人か | 同上 |
| 推移的な依存 — 間接のものも自分の欠陥になる | 同上 |

`escrow-domain` へ足す場合は、`crates/escrow-domain/tests/kernel.rs` の表の編集がそのまま
判断の記録になる。

### 標準の綴りに寄せる

**`std` に同じ綴りがあるなら、それを使う。** `as_str` は `String::as_str` の綴りで、`AsRef` は
generic の境界に使う別の道具。エディションや言語機能の更新で簡潔に書けるようになったものは、
取り込む。

`new` と `From` は
[C-CTOR](https://rust-lang.github.io/api-guidelines/predictability.html) と
[C-CONV-TRAITS](https://rust-lang.github.io/api-guidelines/interoperability.html) の
どちらも相手を否定しておらず、`std` 自身が `String::new` と `String::from` の両方を持つ。
**優劣が書かれていない選択は、下の「同じ仕事は同じ形」で決める。**

### 同じ仕事は同じ形

[**Rust API Guidelines — Predictability**](https://rust-lang.github.io/api-guidelines/predictability.html)。
**単位はワークスペース全体。** 1つの仕事に2つ目の書き方が現れたら、どちらかへ寄せる。

| 仕事 | 形 |
|---|---|
| 値を包む newtype を作る | `X::new(...)` |
| 内側の値を取り出す | `as_str()`（借用）/ `i64::from(x)`（`Copy`） |

**レビューで足すことを検討し、意図的に見送った導出**は、理由をその型の doc に1行残す。
足していない導出すべてが対象ではない。

---

## 守り

[**Poka-yoke**](https://en.wikipedia.org/wiki/Poka-yoke)（新郷重夫, トヨタ生産方式）—
間違いが起きない形に作り替える。**この規約に「Xをするな」と書くだけなら訓練であり、訓練は
劣化する。** 守らせたい約束は、破ったときに落ちる形へ持っていく。

**ワークスペース全体にかかるものは `tests/` に置く。** member の一覧を `Cargo.toml` から
読むので、crate をどこへ置いても届く。1つの crate に閉じたものは、その crate の `tests/` へ。

| 守るもの | 落ち方 | どこ |
|---|---|---|
| `macro_rules!` の禁止 | テスト | `tests/macros.rs` |
| crate の依存の向き、スライス同士の隔離 | テスト | `tests/dependency_direction.rs` |
| カーネルの依存とモジュールの一覧 | テスト | `crates/escrow-domain/tests/kernel.rs` |
| `escrow-ledger` の公開 API、投影へ書く SQL の置き場所 | テスト | `crates/escrow-ledger/tests/public_api.rs` |
| 状態遷移に受け皿（`_ =>`）を置かない | コンパイル | `crates/escrow-domain/src/state.rs` の `next` |
| 図にある遷移がちょうど 20 本（9状態 × 11事象のうち） | テスト | 同 `exactly_the_diagram_is_legal` |
| 事象の順序と競合 | DB の制約 | `item_event(item_id, seq)` の UNIQUE |

---

## 動かす

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo test --workspace --doc      # nextest は doctest を実行しない
```

SQL を変えたら `.sqlx/` を取り直す。手順は `.cargo/config.toml` の先頭にある。

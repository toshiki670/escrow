# escrow 規約

**実装者とレビュアの判断基準。** 何かを書く前と、レビューで判断が割れたときに引く。

**出典の列が `escrow` の規則は、escrow の判断。** それ以外は出典に当たれば根拠が読める。
判断が割れたら出典を見る。

規則の本文は**この文書だけで読める**ようにする。Issue は経緯の保存先で、規則の意味を
そこから補わない。

## 検査の3層

| 層 | 中身 | 破ったとき |
|---|---|---|
| 一般的な品質検査 | `cargo fmt` / `cargo clippy -D warnings` / `cargo nextest` / `cargo test --doc` | CI が落ちる |
| **規約固有の守り** | この規約の一部を機械で見るテスト（「[守り](#守り)」） | CI が落ちる |
| 手動レビュー | 上の2層で見られない規則すべて | レビューで指摘する |

**規約の大半は3層目。** 機械で見られるものは「守り」の表に挙げたものだけで、それ以外は
人が読んで判断する。

---

## 記録の置き場所

| 置き場所 | 残すもの |
|---|---|
| この規約 | 閉じたあとも効き続ける規則 |
| Issue | **何を作るか**の決定と、そこに至る経緯。実装が終われば閉じるが、閉じても読める |
| PR | **どう作ったか。** その差分に固有のこと — 確かめ方と結果、実装中に見つけたこと |
| コードの doc | 次にそこを触る人が間違えること |

| 規則 | 出典 |
|---|---|
| 同じことを2か所に書かない。写しを作ると、片方を直してももう片方が古いまま残る | [DRY](https://pragprog.com/tips/)（*The Pragmatic Programmer*, Tip 15） |
| PR は Issue へリンクし、決定そのものを写さない。実装中に設計の穴が見つかったら、決定は Issue の本文へ書き、PR は**どの Issue のどこを直したか**を指す | escrow |
| 決定には理由を添える。理由が無いと、次の人は盲目的に受け入れるか盲目的に変えるかしかできない | [ADR](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)（Nygard, 2011） |

escrow は ADR をファイルで持たず、閉じた Issue がその役をする。

---

## アーキテクチャ

| 軸 | 採るもの | 出典 |
|---|---|---|
| コードの分け方 | **Vertical Slice** — 技術ではなくライフサイクルの段階で切る | [Bogard, 2018](https://www.jimmybogard.com/vertical-slice-architecture/)「Minimize coupling between slices, and maximize coupling in a slice.」 |
| 状態の持ち方 | **Event Sourcing** — 状態ではなく、状態を変えた事象を保存する | [Fowler, 2005](https://martinfowler.com/eaaDev/EventSourcing.html)「Capture all changes to an application state as a sequence of events.」 |
| 読み書きの分け方 | **CQRS** — 書くのは事象、読むのは投影 | [Young / Fowler, 2011](https://martinfowler.com/bliki/CQRS.html)「you can use a different model to update information than the model you use to read information」 |

段は5つ。**役割で決まる**ので、crate の名前が変わっても動かない。

| 段 | 役割 |
|---|---|
| 5 | 入口。組み立てる |
| 4 | スライス。ライフサイクルの1段階を担う |
| 3 | 外部アクセスの受付 |
| 2 | 永続化・外部ツール・設定 |
| 1 | カーネル。仕様の型と状態機械。同期・純関数だけを置く |

| 規則 | 出典 |
|---|---|
| 下の段しか見えない | escrow |
| 段4 のスライスは互いを知らない。順序はコードの呼び出し順ではなく状態機械が持ち、スライスは投影を状態で絞って拾われる | escrow |
| 外部アクセスの port は、段3 の crate の公開 API。段4・段5 は外部ツールの crate を名前で知らない | escrow |
| 事象を書く道は2つだけ。投影はいつでも捨てて作り直せる | escrow |
| 状態遷移は**全域関数1つが正本**。段1 に置き、受け皿（`_ =>`）を持たない。ここに無い遷移は通らない | escrow |

**どの crate がどの段に居て、どの辺が許されているかの正本は `tests/dependency_direction.rs`。**
この規約に写さない。

---

## ドキュメント

**doc は日本語で書く。** 対象で要求が変わる。

| 対象 | 要求 |
|---|---|
| **crate の外から到達できる** API の rustdoc | 下の「形」と「中身」に従う |
| 外から到達できないもの（`pub(crate)`・private・**private なモジュールの中の `pub`**） | why だけ。契約の節は要らない |
| 通常のコメント（`//`） | why だけ |

### 形（公開 API）

[Rust RFC 1574](https://rust-lang.github.io/rfcs/1574-more-api-documentation-conventions.html)
（[RFC 505](https://rust-lang.github.io/rfcs/0505-api-comment-conventions.html) を改訂）の
うち、言語に依らない部分。

| 規則 | 出典 |
|---|---|
| 要約は1行。rustdoc の一覧に出るのはここだけ | RFC 1574 |
| 言い切りで書く（英語の三人称・現在形にあたるもの） | RFC 1574 を日本語へ読み替え（escrow） |
| `Examples` / `Panics` / `Errors` / `Safety` の節を**書くときは**この名前を使い、1つしか無くても複数形にする。それ以外の見出しは日本語で自由に付けてよい | RFC 1574 |

### 中身（公開 API）

| 規則 | 出典 |
|---|---|
| **該当する契約があるときに限り**、失敗・パニック・安全性を書く。返す型が `Result` でなく、panic も unsafe も無いなら、節を置かない | [C-FAILURE](https://rust-lang.github.io/api-guidelines/documentation.html) |
| 関連するものへリンクを張る | C-LINK |
| 実装の詳細を rustdoc に出さない | C-HIDDEN |

### 何を書くか（すべての対象）

| 規則 | 違反の見分け方 | 出典 |
|---|---|---|
| コメントは **why**、コードが **how** | コードを読めば分かることを日本語にしている | McConnell, *Code Complete* 32章「As you're about to add a comment, ask yourself, 'How can I improve the code so that this comment isn't needed?'」 |
| 補足が増えたら、まず設計を疑う | 置き場所・構造・名前で解けるものを日本語で補っている | escrow |
| 次にここを触る人が間違えることだけ残す | レビューや相談で説明のために作った比較・言い換え・経緯が残っている | escrow |

```rust
// 良い — その形にした理由
/// 投影のスキーマを変えたいときは移行ではなくこれを走らせる。移行に混ぜると、
/// いつの間にか投影に手作業の移行が当たって作り直せなくなる。

// 悪い — コードが言っていることの繰り返し
/// item テーブルを DROP して、CREATE して、ログから INSERT する。

// 悪い — 説明のために作った比較が残っている
/// 識別子の newtype とはそこが違う — あちらは包むのが仕事だが、
/// こちらは作る道を絞るのが仕事。
```

### 言葉（すべての対象）

| 規則 | 違反の見分け方 | 出典 |
|---|---|---|
| 指しているものの名前で書く | 比喩が名前になっている（「口」「窓」） | **Ubiquitous Language**（Evans, *Domain-Driven Design*, 2003） |
| 能動態で書く | 「〜される」など、動作主体を不必要に隠す受動表現になっている | [Google developer documentation style guide](https://developers.google.com/style/voice) |
| 肯定形で書く | 「〜できない」「〜しない」で始まる説明がある。「作る道は2つ」と言い換えられる | 同上（能動態の系） |
| 根拠を仕様に置く | 根拠が依存 crate の API 名や、いまの実装の形になっている（「`sqlx` がこう返すから」） | escrow |
| 推測は結論と同じ段落に置く | 「たぶん」が別の段落にあり、結論だけ読むと確定に見える | escrow |
| 型で締めなかった所は、そう書く | 緩い型に理由が無い。**「決めて緩めた」と「締め忘れた」が区別できなくなる** | escrow |

---

## 実装

### マクロを禁じる

**`macro_rules!` はワークスペース全体で禁じている。** `derive`・generics・trait で書ける形を
採る。**目的に足りる範囲で最も弱い道具を選ぶ**という
[Rule of Least Power](https://www.w3.org/2001/tag/doc/leastPower.html)（W3C TAG, 2006）の
適用で、弱い道具ほど外から中身を読み解ける。

本当に要るものが出てきたときの道は2つ。

1. **escrow から独立した crate にする。** マクロで解くほど一般的な仕組みなら、escrow に
   閉じている理由が無いことが多い
2. **禁止を「限定的な許可」へ書き換える。** 何を許したかと理由がテストに残る

### 依存を採るかどうか

**見送る条件。** どれか1つでも当たれば採らない。

| 条件 | 出典 |
|---|---|
| 置き換える対象に、テストすべき振る舞いが無い — 分岐・変換・境界・単位のどれも無い。`Self(x.into())` と `&self.0` がこれで、採っても行数が減るだけ | escrow |
| 呼ぶ側の負担が増える — 綴りの変更・型注釈が要るようになる・`.to_owned()` が要る | escrow |
| **入れる目的が、いまある API の綴りを変えることだけ**で、かつ公式のガイドラインがいまの綴りを否定していない | escrow |

**調べて PR に残すこと。** **閾値は置かない** — 同じ数字でも crate によって意味が違うので、
見た結果と、そこから何を判断したかを書く。

| 見るもの | 出典 |
|---|---|
| issue tracker — 未解決の数と、放置されている期間 | [Cox, 2019](https://research.swtch.com/deps) |
| commit 履歴 — いま手が入っているか、書き手が何人か | 同上 |
| 推移的な依存 — 間接のものも自分の欠陥になる | 同上 |

段1 のカーネルへ足す場合は、依存の許可リストの編集がそのまま判断の記録になる。

### 綴りと形を揃える

| 規則 | 違反の見分け方 | 出典 |
|---|---|---|
| `std` に同じ綴りがあるなら、それを使う | `as_str` の代わりに `AsRef`、のように std の綴りを避けている | escrow |
| 公式のガイドラインが優劣を書いていない選択は、**いま揃っているほうを崩さない** | 綴りを変える理由が好みだけ | escrow |
| 同じ仕事に2つ目の書き方が現れたら、どちらかへ寄せる。**単位はワークスペース全体** | 値を包む newtype の作り方が2通りある、など | [Rust API Guidelines — Predictability](https://rust-lang.github.io/api-guidelines/predictability.html) |
| **レビューで足すことを検討し、意図的に見送った導出**は、理由をその型の doc に1行残す。足していない導出すべてが対象ではない | 見送りの議論が PR にしか残っていない | escrow |

---

## 守り

**守らせたい約束は、破ったときに落ちる形へ持っていく。** 規約に「Xをするな」と書くだけなら
訓練であり、訓練は劣化する（[Poka-yoke](https://en.wikipedia.org/wiki/Poka-yoke)、新郷重夫）。

**ワークスペース全体にかかるものは `tests/` に置く。** member の一覧を `Cargo.toml` から
読むので、crate をどこへ置いても届く。1つの crate に閉じたものは、その crate の `tests/` へ。

| 守っているもの | 落ち方 |
|---|---|
| `macro_rules!` の禁止 | テスト |
| crate の依存の向き、スライス同士の隔離 | テスト |
| カーネルの依存とモジュールの一覧 | テスト |
| ログと投影を、同じ経路でしか書けないこと | テスト |
| 状態遷移に受け皿（`_ =>`）を置かないこと | コンパイル |
| 状態遷移の全域関数に**無い**遷移が通らず、通る本数が動かないこと | テスト |
| 事象の順序と競合 | DB の制約 |

**それぞれの正本はテスト自身。** 何をどう見ているかはテストを読む。

---

## 動かす

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo test --workspace --doc      # nextest は doctest を実行しない
```

SQL を変えたら `.sqlx/` を取り直す。手順は `.cargo/config.toml` の先頭にある。

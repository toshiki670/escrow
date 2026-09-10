# ドキュメント

[escrow 規約](../../CONTRIBUTING.md) の一部。

**doc は日本語で書く。** 対象で要求が変わる。

| 対象 | 要求 |
|---|---|
| **crate の外から到達できる** API の rustdoc | 下の「形」と「中身」に従う |
| 外から到達できないもの（`pub(crate)`・private・**private なモジュールの中の `pub`**） | why だけ。契約の節は要らない |
| 通常のコメント（`//`） | why だけ |

## 形（公開 API）

[Rust RFC 1574](https://rust-lang.github.io/rfcs/1574-more-api-documentation-conventions.html)
（[RFC 505](https://rust-lang.github.io/rfcs/0505-api-comment-conventions.html) を改訂）の
うち、言語に依らない部分。

| 規則 | 出典 |
|---|---|
| 要約は1行。rustdoc の一覧に出るのはここだけ | RFC 1574 |
| 言い切りで書く（英語の三人称・現在形にあたるもの） | RFC 1574 を日本語へ読み替え（escrow） |
| `Examples` / `Panics` / `Errors` / `Safety` の節を**書くときは**この名前を使い、1つしか無くても複数形にする。それ以外の見出しは日本語で自由に付けてよい | RFC 1574 |

## 中身（公開 API）

| 規則 | 出典 |
|---|---|
| **該当する契約があるときに限り**、失敗・パニック・安全性を書く。返す型が `Result` でなく、panic も unsafe も無いなら、節を置かない | [C-FAILURE](https://rust-lang.github.io/api-guidelines/documentation.html) |
| 関連するものへリンクを張る | C-LINK |
| 実装の詳細を rustdoc に出さない | C-HIDDEN |

## 何を書くか（すべての対象）

| 規則 | 違反の見分け方 | 出典 |
|---|---|---|
| コメントは **why**、コードが **how** | コードを読めば分かることを日本語にしている | McConnell, *Code Complete* 32章「As you're about to add a comment, ask yourself, 'How can I improve the code so that this comment isn't needed?'」 |
| 補足が増えたら、まず設計を疑う | 置き場所・構造・名前で解けるものを日本語で補っている | escrow |
| 次にここを触る人が間違えることだけ残す（「[記録の置き場所](records.md)」） | レビューや相談で説明のために作った比較・言い換え・経緯が残っている | escrow |

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

## 言葉（すべての対象）

**次の6つは、コードの doc とコメントに加えて、Issue・PR・レビュー返信・報告にも掛かる**
— 指しているものの名前で書く / 能動態で書く / 肯定形で書く / 根拠を仕様に置く /
推測は結論と同じ段落に置く / 規約を指すときは、ファイル名と、節が在れば節名、無ければ引用文で指す。

| 規則 | 違反の見分け方 | 出典 |
|---|---|---|
| 指しているものの名前で書く | 比喩が名前になっている（「口」「窓」） | **Ubiquitous Language**（Evans, *Domain-Driven Design*, 2003） |
| 能動態で書く | 「〜される」など、動作主体を不必要に隠す受動表現になっている | [Google developer documentation style guide](https://developers.google.com/style/voice) |
| 肯定形で書く | 「〜できない」「〜しない」で始まる説明がある。「作る道は2つ」と言い換えられる | 同上（能動態の系） |
| 根拠を仕様に置く | 根拠が依存 crate の API 名や、いまの実装の形になっている（「`sqlx` がこう返すから」） | escrow |
| 推測は結論と同じ段落に置く | 「たぶん」が別の段落にあり、結論だけ読むと確定に見える | escrow |
| **規約を指すときは、ファイル名と、節が在れば節名、無ければ引用文で指す** | **いまの**規約を行番号で指している。**過去の観測は、いつの時点かを名指しする** —— その位置を過去にした変更か、その参照を持っていた成果物 | [Cool URIs don't change](https://www.w3.org/Provider/Style/URI)（Berners-Lee, W3C, 1998）「**URIs change when there is some information in them which changes**」。**原文が扱うのは URI。escrow は規約の中の行を指す参照へ当てた（escrow）** |
| 型で締めなかった所は、そう書く | 緩い型に理由が無い。**「決めて緩めた」と「締め忘れた」が区別できなくなる** | escrow |

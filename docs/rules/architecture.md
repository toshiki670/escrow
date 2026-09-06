# アーキテクチャ

[escrow 規約](../../CONTRIBUTING.md) の一部。

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
| 状態遷移は**全域関数1つが正本**。段1 に置き、受け皿（`_ =>`）を持たない。ここに無い遷移はコンパイルが通さない | escrow |

**どの crate がどの段に居て、どの辺が許されているかの正本は `tests/dependency_direction.rs`。**

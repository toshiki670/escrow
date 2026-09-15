# アーキテクチャ

[escrow 規約](../../CONTRIBUTING.md) の一部。

| 軸 | 採るもの | 出典 |
|---|---|---|
| コードの分け方 | **Vertical Slice** — 技術ではなくライフサイクルの段階で切る | [Bogard, 2018](https://www.jimmybogard.com/vertical-slice-architecture/)「Minimize coupling between slices, and maximize coupling in a slice.」「In this style, my architecture is built around distinct **requests**」。**原文が単位に置くのは request。escrow は順序を状態機械が持つので、ライフサイクルの段階を単位にした（escrow）** |
| 状態の持ち方 | **Event Sourcing** — 状態ではなく、状態を変えたイベントを保存する。イベントの表がイベントログ、それを持って追記と読み出しを受けるものがイベントストア | [Fowler, 2005](https://martinfowler.com/eaaDev/EventSourcing.html)「Capture all changes to an application state as a sequence of **events**.」。[Young, 2010](https://cqrs.wordpress.com/wp-content/uploads/2010/11/cqrs_documents.pdf)「This table represents the actual **Event Log**. There will be one entry per event in this table.」「Listing 6 Interface for an **Event Store**」（`IEventStore` の `SaveChanges` / `GetEventsFor`）。**原文のイベントストアが持つのはイベントの表と、そこから導ける aggregate の版の表（Figure 20「it could be derived from the Events table」）で、操作はその2つだけ。escrow のイベントストアはそれに加えて、どのイベントからも導けない catalog（いまの値の行を直接書く）とリードモデルを同じ SQLite に置き、リードモデルへの問い合わせを公開 API に持つ（escrow）** |
| 読み書きの分け方 | **CQRS** — 書くのはイベント、読むのはリードモデル | [Young / Fowler, 2011](https://martinfowler.com/bliki/CQRS.html)「split that conceptual model into separate models for update and display, which it refers to as **Command** and **Query** respectively」「you can structure **read models** as EventPosters」。[Young, 2010](https://cqrs.wordpress.com/wp-content/uploads/2010/11/cqrs_documents.pdf)「The third is the **Read Model**; it consumes events and produces DTOs」。**escrow は Command の model を上の Event Sourcing のイベントで、Query の model をリードモデルで置いた（escrow）** |

段は次のとおり。**役割で決まる**ので、crate の名前が変わっても動かない。

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
| 段4 のスライスは互いを知らない。順序はコードの呼び出し順ではなく状態機械が持ち、スライスはリードモデルを状態で絞って拾われる | escrow |
| 外部アクセスの port は、段3 の crate の公開 API。段4・段5 は外部ツールの crate を名前で知らない | escrow |
| イベントを書く道は2つだけ。リードモデルはいつでも捨てて作り直せる | escrow |
| 状態遷移は**全域関数1つが正本**。段1 に置き、受け皿（`_ =>`）を持たない。ここに無い遷移は通らない | escrow |

**どの crate がどの段に居て、どの辺が許されているかの正本は `tests/dependency_direction.rs`。**

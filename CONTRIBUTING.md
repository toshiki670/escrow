# escrow 規約

**設計・実装・レビュー・QA の判断基準。** 何かを書く前と、レビューで判断が割れたときに引く。

**出典の列が `escrow` の規則は、escrow の判断。** それ以外は出典に当たれば根拠が読める。
判断が割れたら出典を見る。

規則の本文は**この規約のファイル群だけで読める**ようにする。Issue は経緯の保存先で、規則の
意味をそこから補わない。

## 業務と、読む規約

**規則は主題ごとに1ファイルへ置き、業務はこの索引で絞る。** 業務ごとに規則を割ると、複数の
業務が読むものが写しになる。

| 業務 | 読むファイル |
|---|---|
| **設計** | [記録の置き場所](docs/rules/records.md) / [設計](docs/rules/design.md) / [アーキテクチャ](docs/rules/architecture.md) / [ドキュメント](docs/rules/documentation.md) / [この規約を変える](docs/rules/amendment.md) |
| **実装** | [記録の置き場所](docs/rules/records.md) / [アーキテクチャ](docs/rules/architecture.md) / [実装](docs/rules/construction.md) / [ドキュメント](docs/rules/documentation.md) / [動かす](docs/rules/running.md) / [この規約を変える](docs/rules/amendment.md) |
| **レビュー** | **全部** |
| **QA** | [QA](docs/rules/qa.md) / [アーキテクチャ](docs/rules/architecture.md) / [動かす](docs/rules/running.md) / [この規約を変える](docs/rules/amendment.md) |

**レビューだけ絞らない。** 判断が割れたときに引く役なので、全部を引く。

---

## 検査の3層

| 層 | 中身 | 破ったとき |
|---|---|---|
| 一般的な品質検査 | `cargo fmt` / `cargo clippy -D warnings` / `cargo nextest` / `cargo test --doc` | CI が落ちる |
| **規約固有の守り** | この規約の一部を機械で見る守り（「[QA](docs/rules/qa.md)」） | CI が落ちる |
| 手動レビュー | 上の2層で見られない規則すべて | レビューで指摘する |

**規約の大半は3層目。** 機械で見られるものは「[QA](docs/rules/qa.md)」の落ち方に当たる
ものだけで、それ以外は人が読んで判断する。

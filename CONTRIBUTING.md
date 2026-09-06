# escrow 規約

**設計・実装・レビュー・QA の判断基準。** 何かを書く前と、レビューで判断が割れたときに引く。

**出典の列が `escrow` の規則は、escrow の判断。** それ以外は出典に当たれば根拠が読める。
判断が割れたら出典を見る。

**Issue は経緯の保存先。** 規則の意味をそこから補わない。

## 業務と、読むファイル

**業務ごとに、その行のファイルを全部読む。**

| 業務 | 読むファイル |
|---|---|
| **全業務** | このファイル（下の「検査の3層」） |
| **設計** | [記録の置き場所](docs/rules/records.md) / [設計](docs/rules/design.md) / [アーキテクチャ](docs/rules/architecture.md) / [ドキュメント](docs/rules/documentation.md) / [規約の変更](docs/rules/amendment.md) |
| **実装** | [記録の置き場所](docs/rules/records.md) / [アーキテクチャ](docs/rules/architecture.md) / [実装](docs/rules/construction.md) / [ドキュメント](docs/rules/documentation.md) / [手元での実行](docs/rules/running.md) / [規約の変更](docs/rules/amendment.md) |
| **レビュー** | **全部** |
| **QA** | [QA](docs/rules/qa.md) / [アーキテクチャ](docs/rules/architecture.md) / [手元での実行](docs/rules/running.md) / [規約の変更](docs/rules/amendment.md) |

**レビューだけ絞らない。** 判断が割れたときに引く役なので、全部を引く。

---

## 検査の3層

| 層 | 中身 | 破ったとき |
|---|---|---|
| 一般的な品質検査 | `cargo fmt` / `cargo clippy -D warnings` / `cargo nextest` / `cargo test --doc` | CI が落ちる |
| **規約固有の守り** | 規約の一部を機械で見る。作り方は「[QA](docs/rules/qa.md)」 | CI が落ちる |
| 手動レビュー | 上の2層で見られない規則すべて | レビューで指摘する |

**規約の大半は3層目。** 機械で見ているのは規約のごく一部で、残りは人が読んで判断する。

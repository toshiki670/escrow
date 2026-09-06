# この規約を変える

[escrow 規約](../../CONTRIBUTING.md) の一部。

## 変える前に踏む手順

**提案として出し、合意を得てから書く**（RFC プロセス — [IETF RFC 2026](https://www.rfc-editor.org/rfc/rfc2026.html) / Rust RFC）。

1. **規約の中で実現する案を使い切る。** 置き場所の候補を並べ、却下したものは規約のどの行に
   当たるかを書く
2. **既存の実装に前例が無いことを確かめる**（grep）
3. **提案として出す。** この時点で規約ファイルを書き換えない
4. **合意を得てから書く。** 差分は規約だけにする

## 「規約の中では実現できない」と主張するとき

3つを揃える（[Toulmin model](https://www.humanities.mcmaster.ca/~hitchckd/Toulminswarrants.pdf) — Toulmin, *The Uses of Argument*, 1958）。

| Toulmin | 揃えるもの |
|---|---|
| **grounds**（根拠） | 却下した置き場所の候補を表にし、それぞれが規約のどの行に当たるかを書く |
| **backing**（裏づけ） | 同じ問題を既存の実装がどう扱っているか、grep で探した結果 |
| **rebuttal**（反証条件） | この主張が崩れる条件。**依存の図に辺が1本も増えないなら、規約の内側に居る** |

## 差分を分ける

**この規則がかかるファイルは `CONTRIBUTING.md` と `docs/rules/*.md`。** `CLAUDE.md` と
`.claude/**` は規約への参照だけを持ち、規則の本文を持たないので含めない。そこへ規則を
書き足す道は開くが、規約の改竄より軽微で、戻すのも容易だと判断した。

| 規則 | 出典 |
|---|---|
| PR タイトルは Conventional Commits に従う | [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/) |
| 規約ファイルを変える PR の type は `doc` / `docs` | escrow |
| `doc` / `docs` 以外の type の PR は規約ファイルを変えない | escrow |
| 手続きを踏んだ結果を PR 本文に残す。各項目をどう満たしたかを書き、次に変える人が手本にできる形にする | escrow |

## 規約の書き方

| 規則 | 出典 |
|---|---|
| 規則は主題ごとに1ファイルへ置き、業務は `CONTRIBUTING.md` の索引で絞る。業務ごとに規則を割ると、複数の業務が読むものが写しになる | [Information Hiding](https://dl.acm.org/doi/10.1145/361598.361623)（Parnas, 1972） |
| 規則の本文は、規約のファイル群だけで読める形にする | escrow |
| 正本が規約の外にあるものは、規約へ写さず正本を指す | DRY（「[記録の置き場所](records.md)」） |

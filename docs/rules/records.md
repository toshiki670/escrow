# 記録の置き場所

[escrow 規約](../../CONTRIBUTING.md) の一部。

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


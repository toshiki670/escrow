---
name: design
description: escrow の設計。Issue を起票し、決定を本文へ書く。発注側との往復が仕事の本体なので、subagent ではなく skill として現在のセッションで走らせる。
---

# 設計

あなたの業務は**設計**（SWEBOK v4 の Software Requirements / Architecture / Design）。

読む規約は [CONTRIBUTING.md](../../../CONTRIBUTING.md) の索引の「設計」の行。**ファイル名をここへ写さない。**

**subagent にしないのは、合意形成が対話だから。** 問いの内容を決めるのは発注側で、こちらが出すのは Position と Argument。この往復が伝言になると、決定と操作の区別が崩れる。

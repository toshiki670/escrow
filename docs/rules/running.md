# 手元での実行

[escrow 規約](../../CONTRIBUTING.md) の一部。

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo test --workspace --doc      # nextest は doctest を実行しない
```

SQL を変えたら `.sqlx/` を取り直す。手順は `.cargo/config.toml` の先頭にある。

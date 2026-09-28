#!/bin/sh
# UI テストがアプリへ渡す HOME を作る（#106）。中身は `escrow-app` の `App::open_seeded`。
#
#   ui/swift/fixture.sh debug <HOME にするディレクトリ>
#
# 呼ぶのは UI テストのターゲットの phase（project.yml）。DB は作るたびに今のスキーマでできる。
set -eu

profile="${1:?profile は release か debug}"
home="${2:?HOME にするディレクトリ}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

case "$profile" in
  release) cargo_flag=--release ;;
  debug)   cargo_flag= ;;
  *) echo "profile は release か debug: $profile" >&2; exit 1 ;;
esac

cd "$root"

# エディタや Xcode の phase から呼ぶと、PATH に cargo が無い。rustup が置く env を読む。
command -v cargo >/dev/null 2>&1 || . "$HOME/.cargo/env"

# shellcheck disable=SC2086
cargo build --quiet $cargo_flag -p escrow-app --features fixture --bin escrow-fixture

# 前の回の分は消す —— 同じ DB へ2度置くと持ち主が2人ずつになる。
rm -rf "$home"
mkdir -p "$home"
HOME="$home" "$root/target/$profile/escrow-fixture"

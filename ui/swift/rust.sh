#!/bin/sh
# Rust 側を建て、Swift が読む層を生成する（#79）。
#
#   ui/swift/rust.sh release
#   ui/swift/rust.sh debug
#
# 呼ぶのは `build.sh` と、Xcode の pre-build の phase（project.yml）。**両方から呼ぶ**のは、
# Xcode の Run ボタンが Rust 側を建て直さないため —— 生成した層と繋ぐ staticlib の版が食い違うと、
# UniFFI が起動時の照合で止まる（doc コメントも checksum に入る）。
set -eu

profile="${1:?profile は release か debug}"
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

# Rust の C 依存（cc crate 経由）が既定で host の版を向くので、Xcode と同じ下限へ揃える。揃えないと
# 「built for newer macOS version」の警告が object ごとに出る。
export MACOSX_DEPLOYMENT_TARGET=26.0

# CLI も同じ .app に入る（#3）。
# shellcheck disable=SC2086
cargo build $cargo_flag -p escrow-ffi -p escrow-cli

# modulemap のモジュール名は crate 名から付くので、Swift 側の `canImport(EscrowFFI)` に合わせる。
bindgen="cargo run --quiet $cargo_flag -p escrow-ffi --features cli --bin uniffi-bindgen --"
lib="$root/target/$profile/libescrow_ffi.a"
mkdir -p "$here/Sources/EscrowFFI" "$here/Sources/EscrowBindings"
$bindgen --headers "$lib" "$here/Sources/EscrowFFI"
$bindgen --modulemap --module-name EscrowFFI --modulemap-filename module.modulemap "$lib" "$here/Sources/EscrowFFI"
$bindgen --swift-sources "$lib" "$here/Sources/EscrowBindings"

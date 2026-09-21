#!/bin/sh
# escrow の SwiftUI の入口を建て、`.app` へ固める（#79）。
#
#   swift/Escrow/build.sh            # release
#   swift/Escrow/build.sh debug      # Rust も Swift も debug
#
# 手順は cargo → uniffi-bindgen → swift build → .app の順。cargo と Xcode を繋ぐのは
# このファイル1つで、SwiftPM だけで組む。出来上がりは `swift/Escrow/dist/Escrow.app`。
set -eu

profile="${1:-release}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
target="$root/target/$profile"

case "$profile" in
  release) cargo_flag=--release; swift_config=release ;;
  debug)   cargo_flag=;          swift_config=debug ;;
  *) echo "profile は release か debug: $profile" >&2; exit 1 ;;
esac

cd "$root"

# エディタの Run ボタンなど login shell 以外から呼ぶと、PATH に cargo が無い。rustup が置く env を読む。
command -v cargo >/dev/null 2>&1 || . "$HOME/.cargo/env"

# Rust の C 依存（cc crate 経由）が既定で host の版を向くので、Swift と同じ下限へ揃える。揃えないと
# 「built for newer macOS version」の警告が object ごとに出る。
export MACOSX_DEPLOYMENT_TARGET=26.0

# 1. Rust。staticlib（escrow-ffi）と CLI。CLI も同じ .app に入る（#3）。
# shellcheck disable=SC2086
cargo build $cargo_flag -p escrow-ffi -p escrow-cli

# 2. バインディング。staticlib の中の metadata から Swift と C の層を生成する。
#    modulemap のモジュール名は crate 名から付くので、Swift 側の `canImport(EscrowFFI)` に合わせる。
bindgen="cargo run --quiet $cargo_flag -p escrow-ffi --features cli --bin uniffi-bindgen --"
lib="$target/libescrow_ffi.a"
mkdir -p "$here/Sources/EscrowFFI" "$here/Sources/EscrowBindings"
$bindgen --headers "$lib" "$here/Sources/EscrowFFI"
$bindgen --modulemap --module-name EscrowFFI --modulemap-filename module.modulemap "$lib" "$here/Sources/EscrowFFI"
$bindgen --swift-sources "$lib" "$here/Sources/EscrowBindings"

# 3. Swift。Package.swift が $ESCROW_RUST_PROFILE で staticlib の場所を決める。
#    警告を落とすのは `clippy -D warnings` と同じ層（CONTRIBUTING.md「検査の3層」の1層目）。
export ESCROW_RUST_PROFILE="$profile"
swift build -c "$swift_config" --package-path "$here" -Xswiftc -warnings-as-errors
bin="$(swift build -c "$swift_config" --package-path "$here" --show-bin-path)"

# 4. .app。#3 の Cask が要求する形 — Info.plist の CFBundleExecutable が GUI、CLI は同じ MacOS/ に置く。
#    前回の出来上がりの上へ書く。置くものは全部ここに列挙してあるので、古いものは残らない。
app="$here/dist/Escrow.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp -f "$here/Info.plist" "$app/Contents/Info.plist"
cp -f "$bin/escrow-gui" "$app/Contents/MacOS/escrow-gui"
cp -f "$target/escrow" "$app/Contents/MacOS/escrow"
# ad-hoc 署名（#3「署名」）。Apple Silicon が起動するのは署名の有る実行ファイルだけ。
codesign --force --sign - "$app"

echo "$app"

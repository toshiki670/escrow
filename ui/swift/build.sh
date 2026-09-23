#!/bin/sh
# escrow の SwiftUI の入口を建てる（#79）。出来上がった `.app` の場所を標準出力へ出す。
#
#   open "$(ui/swift/build.sh)"          # release
#   open "$(ui/swift/build.sh debug)"    # Rust も Swift も debug
#
# 手順は cargo → uniffi-bindgen → xcodegen → xcodebuild。`.app` の組み立てと署名は Xcode が
# 行い、ここが持つのは Rust 側と、その2つを繋ぐことだけ。`.xcodeproj` は project.yml から
# 毎回作るので commit しない。
set -eu

profile="${1:-release}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

case "$profile" in
  release) cargo_flag=--release; configuration=Release ;;
  debug)   cargo_flag=;          configuration=Debug ;;
  *) echo "profile は release か debug: $profile" >&2; exit 1 ;;
esac

cd "$root"

# エディタの Run ボタンなど login shell 以外から呼ぶと、PATH に cargo が無い。rustup が置く env を読む。
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

xcodegen generate --quiet --spec "$here/project.yml" --project "$here"
mkdir -p "$here/.build"
xcodebuild -project "$here/Escrow.xcodeproj" -scheme Escrow -configuration "$configuration" \
  -derivedDataPath "$here/.build" build > "$here/.build/xcodebuild.log" 2>&1 ||
  { tail -40 "$here/.build/xcodebuild.log" >&2; exit 1; }

echo "$here/.build/Build/Products/$configuration/Escrow.app"

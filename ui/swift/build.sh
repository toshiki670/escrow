#!/bin/sh
# escrow の SwiftUI の入口を建てる（#79）。出来上がった `.app` の場所を標準出力へ出す。
#
#   open "$(ui/swift/build.sh)"          # release
#   open "$(ui/swift/build.sh debug)"    # Rust も Swift も debug
#
# 手順は rust.sh（cargo と uniffi-bindgen）→ xcodegen → xcodebuild。Rust 側を先に通すのは、
# xcodegen が `Sources/` のファイルを数えて project を作るため。`.app` の組み立てと署名は Xcode。
set -eu

profile="${1:-release}"
here="$(cd "$(dirname "$0")" && pwd)"

case "$profile" in
  release) configuration=Release ;;
  debug)   configuration=Debug ;;
  *) echo "profile は release か debug: $profile" >&2; exit 1 ;;
esac

"$here/rust.sh" "$profile"

xcodegen generate --quiet --spec "$here/project.yml" --project "$here"
mkdir -p "$here/.build"
xcodebuild -project "$here/Escrow.xcodeproj" -scheme Escrow -configuration "$configuration" \
  -derivedDataPath "$here/.build" build > "$here/.build/xcodebuild.log" 2>&1 ||
  { tail -40 "$here/.build/xcodebuild.log" >&2; exit 1; }

echo "$here/.build/Build/Products/$configuration/Escrow.app"

#!/bin/sh
# SwiftUI の入口を、アプリを起動して確かめる（#106）。
#
#   ui/swift/test.sh
#
# 建て方は build.sh と同じ Debug。仕込んだ DB は project.yml の phase が作る。
set -eu

here="$(cd "$(dirname "$0")" && pwd)"

"$here/rust.sh" debug

xcodegen generate --quiet --spec "$here/project.yml" --project "$here"
mkdir -p "$here/.build"
xcodebuild -project "$here/Escrow.xcodeproj" -scheme Escrow -configuration Debug \
  -derivedDataPath "$here/.build" test > "$here/.build/xcodebuild-test.log" 2>&1 ||
  { tail -60 "$here/.build/xcodebuild-test.log" >&2; exit 1; }

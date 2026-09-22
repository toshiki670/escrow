// swift-tools-version: 6.2

// escrow の SwiftUI の入口（#79）。Rust 側は `ffi/` で、`build.sh` が staticlib と
// バインディングを作ってからここを建てる。`.xcodeproj` は持たず、SwiftPM だけで組む。

import PackageDescription

/// Rust の staticlib が在る場所。`build.sh` が profile を渡す。
let rustTargetDir =
  Context.packageDirectory + "/../../target/"
  + (Context.environment["ESCROW_RUST_PROFILE"] ?? "release")

let package = Package(
  name: "Escrow",
  platforms: [.macOS(.v26)],
  products: [
    // 実行ファイルの名前は #3 の `.app` の形（`Contents/MacOS/escrow-gui`）に合わせる。`Escrow` にすると、
    // 同じ場所へ置く CLI の `escrow` と APFS（大文字小文字を区別しない）で同じファイルになる。
    .executable(name: "escrow-gui", targets: ["Escrow"])
  ],
  targets: [
    // UniFFI が生成する C の層。header と modulemap は `build.sh` が置く。
    .systemLibrary(name: "EscrowFFI", path: "Sources/EscrowFFI"),
    // UniFFI が生成する Swift の層（bindings）。`Escrow.swift` は `build.sh` が置く。
    .target(
      name: "EscrowBindings",
      dependencies: ["EscrowFFI"],
      path: "Sources/EscrowBindings",
      swiftSettings: [.swiftLanguageMode(.v5)],
      linkerSettings: [
        .unsafeFlags(["-L", rustTargetDir]),
        .linkedLibrary("escrow_ffi"),
        // `cargo rustc -- --print native-static-libs` が挙げたもの。
        .linkedFramework("Security"),
        .linkedFramework("CoreFoundation"),
        .linkedLibrary("iconv"),
      ]
    ),
    .executableTarget(
      name: "Escrow",
      dependencies: ["EscrowBindings"],
      path: "Sources/Escrow",
      swiftSettings: [
        .defaultIsolation(MainActor.self),
        .enableUpcomingFeature("NonisolatedNonsendingByDefault"),
      ]
    ),
  ]
)

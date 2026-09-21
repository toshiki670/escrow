// escrow の SwiftUI の入口（#79）。#6 の骨格を出し、`Person` ごとの項目一覧を見せる。
// Iced 版（`crates/escrow-gui`）と同じ範囲で、読むのはリードモデルだけ。

import SwiftUI

@main
struct EscrowApp: App {
  var body: some Scene {
    WindowGroup {
      ContentView()
    }
    .defaultSize(width: 960, height: 640)
  }
}

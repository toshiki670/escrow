// escrow の SwiftUI の入口（#79）。#6 の骨格を出し、`Person` ごとの項目一覧を見せる。
// 範囲は #30 と同じ（#79「比べるためのもの」）。読むのはリードモデルだけ。

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

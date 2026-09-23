// 画面が持つ状態の型（#6）。イベントストアを開く手順と読む関数は `escrow-app` が持つ（#82）。

import EscrowBindings

/// サイドバーで選べるもの（#6）。
enum Selection: Hashable {
  case dashboard
  case person(PersonId)
  case settings
}

/// `Person` を選んだときのメイン。
enum Listing {
  case loading
  case loaded([Listed])
  case failed(String)
}

/// 画面の全体。イベントストアを開いてから、サイドバーと一覧を並べる。
enum Phase {
  case opening
  /// イベントストアを開けなかった。直す先は設定の場所か DB で、画面はその理由を出すだけ。
  case unavailable(String)
  case ready(Escrow, [Person])
}

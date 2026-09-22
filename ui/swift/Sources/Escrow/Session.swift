// 画面の状態。イベントストアを開く手順と読む関数は `escrow-app` が持ち（#82）、ここに在るのは
// 開いたか・何を選んだか・一覧を読み終えたか、だけ。

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

/// 画面の全体。イベントストアを開くまでは、まだ何も並べられない。
enum Phase {
  case opening
  /// イベントストアを開けなかった。直す先は設定の場所か DB で、画面はその理由を出すだけ。
  case unavailable(String)
  case ready(Escrow, [Person])
}

/// 失敗の理由。`escrow-ffi` は原因まで繋いだ1つの文で返す（`FfiError`）。
func why(_ error: Error) -> String {
  if case FfiError.Failed(let message) = error {
    return message
  }
  return String(describing: error)
}

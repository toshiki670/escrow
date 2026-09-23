import XCTest

/// #30 の受け入れを、描いた画面で見る（#79）。
///
/// 仕込みは偽 HOME で、`xcodebuild test TEST_RUNNER_ESCROW_TEST_HOME=<path>` が渡す。`TEST_RUNNER_`
/// を付けた変数だけがテストランナーへ届く。
@MainActor
final class ListingTests: XCTestCase {
  private func launched() -> XCUIApplication {
    let home = ProcessInfo.processInfo.environment["ESCROW_TEST_HOME"] ?? ""
    XCTAssertFalse(
      home.isEmpty,
      "ESCROW_TEST_HOME がランナーに無い。ランナーが見えている変数: "
        + ProcessInfo.processInfo.environment.keys.sorted().joined(separator: ","))
    let app = XCUIApplication()
    app.launchEnvironment["HOME"] = home
    // 窓の再開を止める。前の回が落ちていると、再開を訊くダイアログが窓の代わりに出る。
    app.launchArguments = ["-ApplePersistenceIgnoreState", "YES"]
    app.launch()
    return app
  }

  /// 画面に出ている文字。SwiftUI の `Text` は文字を `value` に載せるので、identifier では引けない。
  private func text(_ value: String, in app: XCUIApplication) -> XCUIElement {
    app.staticTexts.matching(NSPredicate(format: "value == %@", value)).firstMatch
  }

  func testTheItemsOfTheSelectedPersonAppearInTheList() {
    let app = launched()

    let owner = text("○○", in: app)
    XCTAssertTrue(owner.waitForExistence(timeout: 20), "サイドバーに ○○ が出る")
    owner.click()

    XCTAssertTrue(
      text("transcribing", in: app).waitForExistence(timeout: 20), "状態が #1 の表の値で出る")
    XCTAssertTrue(text("youtube_video", in: app).exists, "種別が #1 の表の値で出る")
    XCTAssertTrue(text("Me at the zoo", in: app).exists, "見出しが出る")
    XCTAssertTrue(text("2005-04-24", in: app).exists, "日付が出る")
  }

  func testAPersonWithoutItemsStillDraws() {
    let app = launched()

    let nobody = text("□□", in: app)
    XCTAssertTrue(nobody.waitForExistence(timeout: 20), "サイドバーに □□ が出る")
    nobody.click()

    XCTAssertTrue(text("項目はまだ無い", in: app).waitForExistence(timeout: 20))
  }
}

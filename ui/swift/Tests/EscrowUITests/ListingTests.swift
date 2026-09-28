import XCTest

/// #30 の受け入れを、描いた画面で見る（#106）。
///
/// 仕込みは `escrow-app` の `App::seeded` と同じ形（○○ は配信1本と投稿1件、□□ は項目を持たない）。
/// `project.yml` の phase が、それを置いた HOME をこの bundle へ入れる。
@MainActor
final class ListingTests: XCTestCase {
  private func launched() throws -> XCUIApplication {
    let seeded = try XCTUnwrap(
      Bundle(for: ListingTests.self).url(forResource: "home", withExtension: nil),
      "仕込んだ HOME が bundle に無い。project.yml の「仕込んだ HOME を作る」を見る")
    // 回ごとに写す。アプリは開いた DB へ書くことがあり、前の回の分を次の回へ持ち越さない。
    let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.copyItem(at: seeded, to: home)

    let app = XCUIApplication()
    app.launchEnvironment["HOME"] = home.path
    // 窓の再開を止める。前の回が落ちていると、再開を訊くダイアログが窓の代わりに出る。
    app.launchArguments = ["-ApplePersistenceIgnoreState", "YES"]
    app.launch()
    return app
  }

  /// 画面に出ている文字。SwiftUI の `Text` は文字を `value` に載せるので、identifier では引けない。
  private func text(_ value: String, in app: XCUIApplication) -> XCUIElement {
    app.staticTexts.matching(NSPredicate(format: "value == %@", value)).firstMatch
  }

  func testTheItemsOfTheSelectedPersonAppearInTheList() throws {
    let app = try launched()

    let owner = text("○○", in: app)
    XCTAssertTrue(owner.waitForExistence(timeout: 20), "サイドバーに ○○ が出る")
    owner.click()

    let live = text("○○の雑談配信", in: app)
    XCTAssertTrue(live.waitForExistence(timeout: 20), "Media の見出しは title")
    let post = text("明日の配信は21時から。", in: app)
    XCTAssertTrue(post.exists, "Post の見出しは body の先頭")

    // 状態と種別は #1 の表の値。
    for value in ["holding", "kept", "youtube_live", "x_post"] {
      XCTAssertTrue(text(value, in: app).exists, "\(value) が出る")
    }

    XCTAssertLessThan(live.frame.minY, post.frame.minY, "新しい項目が上に来る")
  }

  func testAPersonWithoutItemsStillDraws() throws {
    let app = try launched()

    let nobody = text("□□", in: app)
    XCTAssertTrue(nobody.waitForExistence(timeout: 20), "サイドバーに □□ が出る")
    nobody.click()

    XCTAssertTrue(text("項目はまだ無い", in: app).waitForExistence(timeout: 20))
  }
}

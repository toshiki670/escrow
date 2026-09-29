//! [`escrow_app::App::open_seeded`] を呼ぶ。SwiftUI の UI テスト（#106）が、別のプロセスで動く
//! アプリへ仕込みを渡すために使う。
//!
//! ```sh
//! HOME=<空のディレクトリ> escrow-fixture
//! ```
//!
//! `HOME` の下に、アプリが開くのと同じ場所で DB ができる。

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), escrow_app::AppError> {
    escrow_app::App::open_seeded().await?;
    Ok(())
}

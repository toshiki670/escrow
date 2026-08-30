use clap::Parser;

/// 配信元から失われうるものを取り込み、手元に預かる。
#[derive(Debug, Parser)]
#[command(name = "escrow", version, about, long_about = None)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}

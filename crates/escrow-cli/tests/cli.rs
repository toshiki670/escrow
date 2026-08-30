//! バイナリ名とバージョンの結線を、実際に起動して確かめる。
//!
//! ここが壊れると #3 の Cask が PATH へ繋ぐ先と #4 が呼ぶ名前がずれるが、
//! crate 名（escrow-cli）とバイナリ名（escrow）が違うため型では守れない。

use std::process::Command;

/// `[[bin]] name = "escrow"` に対して cargo が渡すパス。
const BIN: &str = env!("CARGO_BIN_EXE_escrow");

#[test]
fn version_reports_the_binary_name_and_package_version() {
    let out = Command::new(BIN)
        .arg("--version")
        .output()
        .expect("escrow を起動できること");

    assert!(out.status.success(), "--version が失敗した: {out:?}");

    let stdout = String::from_utf8(out.stdout).expect("stdout が UTF-8 であること");
    assert_eq!(
        stdout.trim(),
        format!("escrow {}", env!("CARGO_PKG_VERSION")),
    );
}

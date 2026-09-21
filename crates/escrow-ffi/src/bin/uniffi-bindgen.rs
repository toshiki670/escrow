//! Swift のバインディングを生成する。呼び方は `swift/Escrow/build.sh`。

fn main() {
    uniffi::uniffi_bindgen_swift();
}

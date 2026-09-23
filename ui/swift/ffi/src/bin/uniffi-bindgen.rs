//! Swift のバインディングを生成する。呼び方は `ui/swift/build.sh`。

fn main() {
    uniffi::uniffi_bindgen_swift();
}

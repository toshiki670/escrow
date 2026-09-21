//! #1 の形式仕様。`Item` の形と、その状態機械。
//!
//! **同期・純関数だけを置く。** DB もランタイムも依存に入らないので、副作用を持つ
//! 便利関数はここに書けない。
//!
//! 置いてよい依存とモジュールは `tests/kernel.rs` の表が決める。モジュールの入場条件は
//! **仕様に載っていること**で、足すには表を編集するしかない。

pub mod asset;
pub mod content;
pub mod item;
pub mod liveness;
pub mod source;
pub mod state;
pub mod timestamp;
pub mod url;

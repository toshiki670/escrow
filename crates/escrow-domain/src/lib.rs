//! #1 の形式仕様。`Item` の形と、その状態機械。
//!
//! **同期・純関数だけを置く。** 依存は `thiserror` / `chrono` / `url` の3つで、
//! DB もランタイムも入らないので、副作用を持つ便利関数はここには書けない（#15）。
//!
//! 置いてよいモジュールは `tests/kernel.rs` の表が決める。入場条件は
//! **#1 に載っていること**で、足すには表を編集するしかない。

mod id;

pub mod asset;
pub mod content;
pub mod item;
pub mod liveness;
pub mod source;
pub mod state;
pub mod timestamp;
pub mod url;

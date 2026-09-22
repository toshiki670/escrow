//! 人が編集する設定 — `person` · `source` · `exclude`（#15）。
//!
//! **いまの値だけを持つ。** 状態機械を持たず、履歴を残す理由も無いので、イベントにはしない。
//! イベントの側と混ざらないように、集約ではなくこの1つのモジュールにまとめる。

mod exclude;
mod person;
mod source;

pub use source::NewSource;

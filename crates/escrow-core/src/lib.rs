//! escrow の中身。取得・預かり・状態遷移・永続化を持ち、CLI と GUI はここを呼ぶ。
//!
//! ここに置く型が #1 の形式仕様にあたる。DB は平らなカラムを持つだけで、
//! 「`Media` に `body` は無い」といった保証はすべてこちらの型が担う。
//!
//! parse を持つ境界は3つだけ — DB の行・外部ツールの出力・人の入力。
//! 境界で型のある値へ写し、写した先では全域関数だけで扱う。

pub mod asset;
pub mod content;
pub mod item;
pub mod liveness;
pub mod source;
pub mod state;
pub mod store;
pub mod timestamp;
pub mod url;

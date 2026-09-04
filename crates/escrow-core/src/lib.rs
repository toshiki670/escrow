//! 取得・預かり・永続化。CLI と GUI はここを呼ぶ。
//!
//! **この crate は Phase 4.3 で消える。** #1 の形式仕様は [`escrow_domain`] へ移り、
//! ここに残っているのは移し先がまだ無いもの（設定・永続化・引き渡し・パイプライン）
//! だけになった。語彙の再公開は、移動を1段で終わらせるための足場（#7）。

pub use escrow_domain::{asset, content, item, liveness, source, state, timestamp, url};

pub mod adapter;
pub mod config;
pub mod handover;
pub mod pipeline;
pub mod store;

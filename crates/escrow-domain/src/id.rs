//! 識別子の newtype を作るマクロ。
//!
//! `i64` の主キーを4つ持つが、どれも取り違えると別のものを指す。裸の `i64` で
//! 回さないための型を、同じ形で作る。

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(i64);

        impl $name {
            pub const fn new(id: i64) -> Self {
                Self(id)
            }

            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

pub(crate) use id_type;

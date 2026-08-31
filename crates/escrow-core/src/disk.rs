//! 空き容量の門。#2 の `storage.min_free_gb`。
//!
//! 取得を**始める前**に見る。始めてから足りなくなると、途中まで書かれた実体と
//! `acquiring` のまま止まった行が残る。門を閉じている間、項目は `waiting` に
//! 留まるだけで、失敗として数えない — 空きが戻れば同じ項目がそのまま流れる。

use std::io;
use std::path::Path;

/// 1GB。設定は GB で書き、比較はバイトで行う。
const GB: u64 = 1024 * 1024 * 1024;

/// 空き容量を見た結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    /// 取得を始めてよい。
    Enough,
    /// 足りない。人が消すか設定を下げるまで、取得は始まらない。
    Short { available: u64, required: u64 },
}

impl Room {
    /// 実測値と設定を突き合わせる。
    ///
    /// 判定はここだけ。GB とバイトの変換を各所でやると、片方だけ直し損ねる。
    pub const fn decide(available: u64, min_free_gb: u64) -> Self {
        let required = min_free_gb.saturating_mul(GB);
        if available >= required {
            Self::Enough
        } else {
            Self::Short {
                available,
                required,
            }
        }
    }

    pub const fn is_enough(self) -> bool {
        matches!(self, Self::Enough)
    }
}

impl std::fmt::Display for Room {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enough => f.write_str("空きは足りている"),
            Self::Short {
                available,
                required,
            } => write!(
                f,
                "空きが足りない（{:.1}GB / {:.1}GB 必要）",
                *available as f64 / GB as f64,
                *required as f64 / GB as f64
            ),
        }
    }
}

/// メディアの置き場所がある区画の空きを見る。
///
/// 区画を自分で探さない。`media_dir` は #2 のとおり外付けにも置けるので、
/// マウント位置と突き合わせる処理を書くと環境差で外す。パスを渡せば
/// そのパスが属する区画を答えるものが `fs4` にあるので、それに任せる。
///
/// `dir` が無いと空きを訊けないので、呼ぶ側が先に作る。
pub fn room(dir: &Path, min_free_gb: u64) -> io::Result<Room> {
    Ok(Room::decide(fs4::available_space(dir)?, min_free_gb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_opens_at_exactly_the_configured_amount() {
        assert_eq!(Room::decide(20 * GB, 20), Room::Enough);
        assert_eq!(
            Room::decide(20 * GB - 1, 20),
            Room::Short {
                available: 20 * GB - 1,
                required: 20 * GB,
            }
        );
        assert_eq!(Room::decide(21 * GB, 20), Room::Enough);
    }

    /// 0 なら門を置かない。
    #[test]
    fn zero_lets_everything_through() {
        assert_eq!(Room::decide(0, 0), Room::Enough);
    }

    /// GB からバイトへの掛け算で回り込まない。回り込むと、巨大な設定が
    /// 「空きは足りている」に化ける。
    #[test]
    fn an_absurd_setting_closes_the_gate_instead_of_wrapping() {
        assert!(!Room::decide(u64::MAX - 1, u64::MAX).is_enough());
    }

    #[test]
    fn reads_the_real_partition() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(room(dir.path(), 0).unwrap(), Room::Enough);
        // どの区画にもこれだけの空きは無い。
        assert!(!room(dir.path(), u64::MAX).unwrap().is_enough());
    }

    /// 場所が無ければ、空いているとは答えない。
    #[test]
    fn a_missing_directory_is_an_error_not_an_open_gate() {
        let dir = tempfile::tempdir().unwrap();
        assert!(room(&dir.path().join("まだ無い"), 0).is_err());
    }
}

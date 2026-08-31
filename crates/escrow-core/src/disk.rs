//! 空き容量の門。#2 の `storage.min_free_gib`。
//!
//! 取得を**始める前**に見る。始めてから足りなくなると、途中まで書かれた実体と
//! `acquiring` のまま止まった行が残る。門を閉じている間、項目は `waiting` に
//! 留まるだけで、失敗として数えない — 空きが戻れば同じ項目がそのまま流れる。

use std::io;
use std::path::Path;

use bytesize::ByteSize;

/// 空き容量を見た結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    /// 取得を始めてよい。
    Enough,
    /// 足りない。人が消すか設定を下げるまで、取得は始まらない。
    Short {
        available: ByteSize,
        required: ByteSize,
    },
}

impl Room {
    /// 実測値と設定を突き合わせる。
    ///
    /// 判定はここだけ。単位の換算を各所でやると、片方だけ直し損ねる。
    ///
    /// 換算するのは、[`available_space`](fs4::available_space) がバイトを返し、
    /// 設定が GiB の個数だからで、比べるには片方を相手の単位へ寄せるしかない。
    /// **上へ寄せる。** 下へ寄せると整数の割り算で端数が落ち、`Short` として
    /// 人へ出す実測値もバイトのままでは出せなくなる。
    ///
    /// 掛け算そのものは [`ByteSize::gib`] に任せる。設定が `u32` なので
    /// **桁あふれる組み合わせが存在しない** — `u32::MAX` GiB でも `u64` に 3 倍の
    /// 余裕で収まる。飽和も検査も要らず、この換算は全域関数になる。
    pub const fn decide(available: u64, min_free_gib: u32) -> Self {
        let required = ByteSize::gib(min_free_gib as u64).as_u64();

        if available >= required {
            Self::Enough
        } else {
            Self::Short {
                available: ByteSize(available),
                required: ByteSize(required),
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
            // `ByteSize` の既定は 2 進表記なので、`GiB` と出る。判定に使った
            // 単位がそのまま人へ出るので、表示と実際がずれない。
            Self::Short {
                available,
                required,
            } => write!(f, "空きが足りない（{available} / {required} 必要）"),
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
pub fn room(dir: &Path, min_free_gib: u32) -> io::Result<Room> {
    Ok(Room::decide(fs4::available_space(dir)?, min_free_gib))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_opens_at_exactly_the_configured_amount() {
        let twenty = ByteSize::gib(20).as_u64();

        assert_eq!(Room::decide(twenty, 20), Room::Enough);
        assert_eq!(
            Room::decide(twenty - 1, 20),
            Room::Short {
                available: ByteSize(twenty - 1),
                required: ByteSize(twenty),
            }
        );
        assert_eq!(Room::decide(twenty + 1, 20), Room::Enough);
    }

    /// 設定は 2 進の GiB。10 進の GB と取り違えると 7% ずれる。
    #[test]
    fn the_unit_is_binary_not_decimal() {
        assert_eq!(bytesize::GIB, 1_073_741_824);
        assert_eq!(bytesize::GB, 1_000_000_000);

        // 20GB ちょうどでは 20GiB に届かない。
        assert!(!Room::decide(ByteSize::gb(20).as_u64(), 20).is_enough());
    }

    /// 出る単位と、判定に使った単位が同じであること。
    #[test]
    fn the_message_names_the_unit_it_judged_by() {
        let short = Room::decide(0, 20);
        assert!(short.to_string().contains("20.0 GiB"), "{short}");
    }

    /// 0 なら門を置かない。
    #[test]
    fn zero_lets_everything_through() {
        assert_eq!(Room::decide(0, 0), Room::Enough);
    }

    /// **設定できるどの値も、バイトへ直せること。**
    ///
    /// `u32` にしたのはこのため。回り込む組み合わせが存在しないので、
    /// 「巨大な設定が『空きは足りている』に化ける」経路そのものが無い。
    #[test]
    fn every_setting_that_can_be_written_can_be_converted() {
        let biggest = Room::decide(u64::MAX, u32::MAX);
        // 1024 EiB 要求して 16 EiB しか無ければ足りない、が正しく出る。
        assert!(!Room::decide(bytesize::EIB, u32::MAX).is_enough());
        // 桁あふれていれば、ここが Short に化ける。
        assert!(biggest.is_enough());
    }

    #[test]
    fn reads_the_real_partition() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(room(dir.path(), 0).unwrap(), Room::Enough);
        // どの区画にもこれだけの空きは無い。
        assert!(!room(dir.path(), u32::MAX).unwrap().is_enough());
    }

    /// 場所が無ければ、空いているとは答えない。
    #[test]
    fn a_missing_directory_is_an_error_not_an_open_gate() {
        let dir = tempfile::tempdir().unwrap();
        assert!(room(&dir.path().join("まだ無い"), 0).is_err());
    }
}

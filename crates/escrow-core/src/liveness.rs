//! 生存確認。`holding` の項目が配信元から消えたかを確かめる（#5）。
//!
//! yt-dlp はどの失敗でも終了コードが 1 になるので、エラー文言を分類しても当てにならない。
//! **分類ではなく非対称性で決める** — 誤って捨てると手元のメディアは戻らないが、
//! 誤って保留すると預かりが延びるだけなので、間違える向きを片側へ寄せる。
//!
//! 実際に配信元を叩く仕事は Phase 4 で足す。ここに置くのは、その結果を表す語彙と、
//! 「在ることを確かめた」という証だけ。

/// 生存確認 1 回ぶんの観測。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// 終了コード 0 で `availability` が取れた。
    Present,
    /// 消えたと断定できた。
    Gone,
    /// 判定保留。非 0、未知のメッセージ、ネットワーク障害はすべてここ。
    Unknown,
}

impl Presence {
    /// 「在る」を観測したときだけ証を返す。
    ///
    /// [`PresenceConfirmed`] を作る道はここしかないので、`Unknown` を握りつぶして
    /// 期限切れを捨てに行くコードは**書けない**。
    pub const fn confirmed(self) -> Option<PresenceConfirmed> {
        match self {
            Self::Present => Some(PresenceConfirmed(())),
            Self::Gone | Self::Unknown => None,
        }
    }
}

/// 配信元に在ることを確かめた証。
///
/// [`Presence::confirmed`] からしか作れない。`holding` から `discarded` へ進む事象が
/// これを要求することで、#1 の「期限まで**在った**」— 沈黙は確認ではない — が
/// 約束ではなく型で効く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceConfirmed(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_positive_observation_yields_the_witness() {
        assert!(Presence::Present.confirmed().is_some());
        assert!(Presence::Gone.confirmed().is_none());
        assert!(Presence::Unknown.confirmed().is_none());
    }
}

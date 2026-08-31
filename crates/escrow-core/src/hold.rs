//! 預かりの期限。`holding` の項目をいつまで持つか。

use std::num::NonZeroU32;

use chrono::TimeDelta;

use crate::timestamp::Timestamp;

/// 預かりの期限。`Item.state_since` と `Source.hold_days` から出る。
///
/// **2つの行にまたがる計算**なので、型にする。`item` の1行だけを見ても期限は
/// 分からず、`source` の1行だけを見ても分からない。突き合わせを式のまま各所へ
/// 散らすと、片方だけ取り違えても誰も気づかない。
///
/// `hold_days` を [`NonZeroU32`] で受けるので、`hold_days` が空の配信元 —
/// #1 の「捨てない」— には期限を作れない。捨てないものを捨てにいく経路が、
/// 約束ではなく型で塞がる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldDeadline(Timestamp);

impl HoldDeadline {
    /// `holding` になった時刻から `hold_days` 後。
    pub fn new(state_since: Timestamp, hold_days: NonZeroU32) -> Self {
        let days = TimeDelta::try_days(i64::from(hold_days.get())).unwrap_or(TimeDelta::MAX);
        Self(state_since + days)
    }

    pub const fn at(self) -> Timestamp {
        self.0
    }

    /// 期限を過ぎているときだけ証を返す。
    ///
    /// [`DeadlineReached`] を作る道はここしかない。
    pub fn reached(self, now: Timestamp) -> Option<DeadlineReached> {
        (now >= self.0).then_some(DeadlineReached(()))
    }
}

/// 預かりの期限が過ぎたことを確かめた証。
///
/// [`crate::liveness::PresenceConfirmed`] と対で
/// [`crate::state::Event::HeldToDeadline`] が要求する。#1 の「期限まで**在った**」は
/// 2つの条件の連言なので、証も2つ要る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlineReached(());

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    fn days(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).expect("テストの日数は 1 以上")
    }

    #[test]
    fn the_deadline_is_hold_days_after_it_became_holding() {
        let deadline = HoldDeadline::new(at("2026-03-01T20:00:00+09:00"), days(7));
        assert_eq!(deadline.at().to_text(), "2026-03-08T20:00:00+09:00");
    }

    #[test]
    fn the_witness_appears_only_once_the_deadline_has_passed() {
        let deadline = HoldDeadline::new(at("2026-03-01T20:00:00+09:00"), days(7));

        assert!(deadline.reached(at("2026-03-08T19:59:59+09:00")).is_none());
        // ちょうど期限は「過ぎた」側。
        assert!(deadline.reached(at("2026-03-08T20:00:00+09:00")).is_some());
        assert!(deadline.reached(at("2026-03-09T00:00:00+09:00")).is_some());
    }

    /// 判定は瞬間で、字面ではない。
    #[test]
    fn compares_instants_across_offsets() {
        let deadline = HoldDeadline::new(at("2026-03-01T20:00:00+09:00"), days(1));
        // 2026-03-02T20:00:00+09:00 と同じ瞬間。
        assert!(deadline.reached(at("2026-03-02T11:00:00+00:00")).is_some());
        assert!(deadline.reached(at("2026-03-02T10:59:59+00:00")).is_none());
    }
}

//! 日時。#1 のとおり ISO 8601 の text で持つ（SQLite に専用の型が無いため）。

use std::fmt;

use chrono::{DateTime, FixedOffset, Local, SecondsFormat, SubsecRound};
use thiserror::Error;

/// 時差を保つ日時。秒までで丸める。
///
/// 配信元がくれる `published_at` は秒精度で、`state_since` にそれ以上の分解能は要らない。
/// 丸めておくと text との往復が字面ごと一致するので、読み書きに揺れが出ない。
///
/// 時差を UTC へ畳まないのは、#4 が `2026-03-01T20:00:00+09:00` の形で外へ返すため。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(DateTime<FixedOffset>);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("ISO 8601 の日時として読めない: {0}")]
pub struct TimestampError(pub String);

impl Timestamp {
    pub fn now() -> Self {
        Self::from(Local::now().fixed_offset())
    }

    /// text から読み戻す。DB の行と外部ツールの出力の両方がここを通る。
    pub fn parse(text: &str) -> Result<Self, TimestampError> {
        DateTime::parse_from_rfc3339(text)
            .map(Self::from)
            .map_err(|_| TimestampError(text.to_owned()))
    }

    /// DB と #4 の JSON に出る形。
    pub fn to_text(self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Secs, false)
    }

    pub const fn inner(self) -> DateTime<FixedOffset> {
        self.0
    }
}

impl From<DateTime<FixedOffset>> for Timestamp {
    fn from(value: DateTime<FixedOffset>) -> Self {
        Self(value.trunc_subsecs(0))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_text() {
        for text in [
            "2026-03-01T20:00:00+09:00",
            "2026-03-01T12:00:00+00:00",
            "1999-12-31T23:59:59-05:00",
        ] {
            let parsed = Timestamp::parse(text).expect(text);
            assert_eq!(parsed.to_text(), text, "字面ごと往復すること");
        }
    }

    /// 時差は畳まない。#4 が受け取る側へそのまま渡すため。
    #[test]
    fn keeps_the_offset_it_was_given() {
        let jst = Timestamp::parse("2026-03-01T20:00:00+09:00").unwrap();
        let utc = Timestamp::parse("2026-03-01T11:00:00+00:00").unwrap();

        assert_eq!(jst.inner(), utc.inner(), "同じ瞬間を指す");
        assert_ne!(jst.to_text(), utc.to_text(), "字面は違う");
    }

    /// 秒より下は落とす。落とさないと往復で字面が変わる。
    #[test]
    fn truncates_below_seconds() {
        let t = Timestamp::parse("2026-03-01T20:00:00.123456+09:00").unwrap();
        assert_eq!(t.to_text(), "2026-03-01T20:00:00+09:00");
    }

    #[test]
    fn rejects_what_is_not_a_timestamp() {
        for text in ["", "2026-03-01", "きのう", "2026-13-01T00:00:00+09:00"] {
            assert!(
                Timestamp::parse(text).is_err(),
                "通ってはいけない: {text:?}"
            );
        }
    }

    #[test]
    fn orders_by_instant_not_by_text() {
        let earlier = Timestamp::parse("2026-03-01T20:00:00+09:00").unwrap();
        let later = Timestamp::parse("2026-03-01T13:00:00+00:00").unwrap();
        assert!(earlier < later);
    }
}

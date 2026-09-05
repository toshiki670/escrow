//! 配信元と、その持ち主、取り込まない種別。#1 の `Person` / `Source` / `Exclude`。

use std::num::NonZeroU32;

use thiserror::Error;

use derive_more::{Constructor, Display, Into};

use crate::content::ContentType;
use crate::state::{Hold, HoldTooFar};
use crate::timestamp::Timestamp;
use crate::url::NormalizedUrl;

/// 配信元の持ち主の識別子。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Constructor, Display, Into)]
pub struct PersonId(i64);

/// 配信元の識別子。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Constructor, Display, Into)]
pub struct SourceId(i64);

/// 除外条件の識別子。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Constructor, Display, Into)]
pub struct ExcludeId(i64);

/// 配信元の持ち主。同じ人物の YouTube チャンネルと X アカウントを束ねる主語（#1）。
///
/// 1つの配信に誰が出ていたかは escrow の責務ではないので、ここは持ち主に留まる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub id: PersonId,
    pub name: String,
}

/// 監視対象。プラットフォーム上の1つのアカウント。
///
/// 持ち主のいない `Source` は作れない（#1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: SourceId,
    pub person_id: PersonId,
    /// 不変 ID へ寄せた URL。ハンドルは改名されうるので持たない（#1）。
    pub url: NormalizedUrl,
    pub enabled: bool,
    /// 登録日時。これ以降の投稿を監視する。
    pub created_at: Timestamp,
    /// 預かる日数。空なら捨てない。
    pub hold_days: Option<NonZeroU32>,
    /// 検知の重み。
    ///
    /// 間隔ではない。間隔を宣言すると、配信元 N 本ぶんの合計が #13 の予算を超えた
    /// 時点で守れない約束になる。重みなら、実際の頻度は予算から導出される（#1）。
    pub priority: NonZeroU32,
    /// いつからいつまで見るか。
    pub monitoring: Monitoring,
}

/// 監視の期間（#1）。
///
/// X は継続監視せず、人が宣言した期間の中だけ見る（#5）。YouTube は RSS が
/// 1チャンネル1回で済むので区切らない。
///
/// **意味があるのは「両方 NULL」と「両方埋まっている」の2つだけ。** DB の
/// `monitor_from` / `monitor_until` は2列に分かれているが、写した先では1つの値にする。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Monitoring {
    /// 区切らず監視し続ける。
    Continuous,
    /// この期間の中だけ見る。
    Period { from: Timestamp, until: Timestamp },
}

/// 監視の期間として読めなかった。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MonitoringError {
    #[error("監視の開始と終了は、両方を決めるか、両方を空にする")]
    HalfOpen,
    #[error("監視の終了は開始より後")]
    NotOrdered,
}

impl Monitoring {
    /// 2つの値から組み立てる。DB の行と人の入力が通る唯一の入口。
    pub fn new(from: Option<Timestamp>, until: Option<Timestamp>) -> Result<Self, MonitoringError> {
        match (from, until) {
            (None, None) => Ok(Self::Continuous),
            (Some(from), Some(until)) if from < until => Ok(Self::Period { from, until }),
            (Some(_), Some(_)) => Err(MonitoringError::NotOrdered),
            (Some(_), None) | (None, Some(_)) => Err(MonitoringError::HalfOpen),
        }
    }

    /// その時刻が監視の中か。
    ///
    /// 期間を持たない配信元は常に中。期間を持つ配信元は、始まりを含み終わりを
    /// 含まない半開区間で見る — 終わりの時刻に2回見る形にしないため。
    pub fn covers(self, now: Timestamp) -> bool {
        match self {
            Self::Continuous => true,
            Self::Period { from, until } => from <= now && now < until,
        }
    }

    /// 書き戻すときの2列。
    pub const fn columns(self) -> (Option<Timestamp>, Option<Timestamp>) {
        match self {
            Self::Continuous => (None, None),
            Self::Period { from, until } => (Some(from), Some(until)),
        }
    }
}

impl Source {
    /// 取得が終わった時点で確定する、預かりの期限（#1）。
    ///
    /// `hold_days` は「これから取得するものを何日預かるか」の**既定値**で、ここで
    /// 1つの日時になったあとは `Item` が持つ。後から日数を変えても、進行中の
    /// 預かりは動かない。
    ///
    /// **渡す時刻は取得が終わった瞬間。** 取得を始めるときに前もって出しておくと、
    /// 数時間の録画ではその長さぶん期限が早まる。
    pub fn hold_from(&self, acquired_at: Timestamp) -> Result<Hold, HoldTooFar> {
        Hold::from_days(self.hold_days, acquired_at)
    }
}

/// 取り込まない種別。`Source` ごと、または全対象共通（#1）。
///
/// 当たったものは `Item` に行を作らない。除外されていることはこちらが持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exclude {
    pub id: ExcludeId,
    /// どの `Source` の除外条件か。空なら全対象に効く共通条件。
    pub source_id: Option<SourceId>,
    pub content_type: ContentType,
    pub enabled: bool,
}

impl Exclude {
    /// この条件が、その配信元のその種別に効くか。
    pub fn covers(&self, source_id: SourceId, content_type: ContentType) -> bool {
        self.enabled
            && self.content_type == content_type
            && self.source_id.is_none_or(|scoped| scoped == source_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(hold_days: Option<u32>) -> Source {
        Source {
            id: SourceId::new(1),
            person_id: PersonId::new(1),
            url: crate::url::normalize_source(
                "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
            )
            .unwrap(),
            enabled: true,
            created_at: Timestamp::parse("2026-01-01T00:00:00+09:00").unwrap(),
            hold_days: hold_days.map(|d| NonZeroU32::new(d).unwrap()),
            priority: NonZeroU32::new(1).unwrap(),
            monitoring: Monitoring::Continuous,
        }
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    /// 「両方が空なら継続して監視する」（#1）。
    #[test]
    fn an_empty_period_means_watching_without_end() {
        assert_eq!(Monitoring::new(None, None), Ok(Monitoring::Continuous));
    }

    #[test]
    fn a_period_round_trips_through_its_two_columns() {
        let from = at("2026-09-01T00:00:00+09:00");
        let until = at("2026-09-08T00:00:00+09:00");

        let monitoring = Monitoring::new(Some(from), Some(until)).unwrap();
        assert_eq!(monitoring, Monitoring::Period { from, until });
        assert_eq!(monitoring.columns(), (Some(from), Some(until)));
        assert_eq!(Monitoring::Continuous.columns(), (None, None));
    }

    /// 片方だけ決まった行は意味が決まっていないので、型にできない。
    #[test]
    fn half_of_a_period_is_not_a_period() {
        let at = at("2026-09-01T00:00:00+09:00");

        assert_eq!(
            Monitoring::new(Some(at), None),
            Err(MonitoringError::HalfOpen)
        );
        assert_eq!(
            Monitoring::new(None, Some(at)),
            Err(MonitoringError::HalfOpen)
        );
    }

    #[test]
    fn a_period_that_ends_before_it_starts_is_not_a_period() {
        let from = at("2026-09-08T00:00:00+09:00");
        let until = at("2026-09-01T00:00:00+09:00");

        assert_eq!(
            Monitoring::new(Some(from), Some(until)),
            Err(MonitoringError::NotOrdered)
        );
        assert_eq!(
            Monitoring::new(Some(from), Some(from)),
            Err(MonitoringError::NotOrdered)
        );
    }

    /// #1 の「`hold_days` が空なら捨てない」と、期限が取得時に確定すること。
    #[test]
    fn the_deadline_is_fixed_at_the_moment_of_acquisition() {
        let at = Timestamp::parse("2026-03-01T22:30:00+09:00").unwrap();

        assert_eq!(
            source(Some(7)).hold_from(at).unwrap(),
            Hold::Until(Timestamp::parse("2026-03-08T22:30:00+09:00").unwrap()),
        );
        assert_eq!(source(None).hold_from(at).unwrap(), Hold::None);
    }

    /// 期限なしと、暦の外は別のこと。握りつぶすと「捨てない」に化ける。
    #[test]
    fn a_span_outside_the_calendar_is_not_the_same_as_no_deadline() {
        let at = Timestamp::parse("2026-03-01T22:30:00+09:00").unwrap();
        let mut absurd = source(Some(7));
        absurd.hold_days = Some(NonZeroU32::MAX);

        assert!(absurd.hold_from(at).is_err());
    }

    /// 監視の期間の外では見ない（#1・#5）。
    #[test]
    fn a_period_is_half_open() {
        let from = Timestamp::parse("2026-09-01T00:00:00+09:00").unwrap();
        let until = Timestamp::parse("2026-09-08T00:00:00+09:00").unwrap();
        let period = Monitoring::new(Some(from), Some(until)).unwrap();

        assert!(period.covers(from), "始まりは中");
        assert!(period.covers(Timestamp::parse("2026-09-07T23:59:59+09:00").unwrap()));
        assert!(!period.covers(until), "終わりは外");
        assert!(!period.covers(Timestamp::parse("2026-08-31T23:59:59+09:00").unwrap()));

        assert!(Monitoring::Continuous.covers(from), "期間が無ければ常に中");
    }

    #[test]
    fn a_scoped_exclude_only_covers_its_own_source() {
        let exclude = Exclude {
            id: ExcludeId::new(1),
            source_id: Some(SourceId::new(1)),
            content_type: ContentType::XSpace,
            enabled: true,
        };

        assert!(exclude.covers(SourceId::new(1), ContentType::XSpace));
        assert!(!exclude.covers(SourceId::new(2), ContentType::XSpace));
        assert!(!exclude.covers(SourceId::new(1), ContentType::XPost));
    }

    /// `source_id` が空なら全対象に効く（#1 の `NULL` の意味）。
    #[test]
    fn a_shared_exclude_covers_every_source() {
        let exclude = Exclude {
            id: ExcludeId::new(1),
            source_id: None,
            content_type: ContentType::XSpace,
            enabled: true,
        };

        assert!(exclude.covers(SourceId::new(1), ContentType::XSpace));
        assert!(exclude.covers(SourceId::new(99), ContentType::XSpace));
        assert!(!exclude.covers(SourceId::new(1), ContentType::YoutubeLive));
    }

    #[test]
    fn a_disabled_exclude_covers_nothing() {
        let exclude = Exclude {
            id: ExcludeId::new(1),
            source_id: None,
            content_type: ContentType::XSpace,
            enabled: false,
        };

        assert!(!exclude.covers(SourceId::new(1), ContentType::XSpace));
    }
}

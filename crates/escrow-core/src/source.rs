//! 配信元と、その持ち主、取り込まない種別。#1 の `Person` / `Source` / `Exclude`。

use std::num::NonZeroU32;

use crate::content::ContentType;
use crate::id::id_type;
use crate::state::HoldPolicy;
use crate::timestamp::Timestamp;
use crate::url::NormalizedUrl;

id_type! {
    /// 配信元の持ち主の識別子。
    PersonId
}
id_type! {
    /// 配信元の識別子。
    SourceId
}
id_type! {
    /// 除外条件の識別子。
    ExcludeId
}

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
    /// 新規投稿とライブ開始を確認する間隔。
    ///
    /// 生存確認（共通設定）とは別の概念なので、`Source` ごとに持つ。X は配信が
    /// 予約されず突発的に始まるので高頻度、YouTube は予約枠が先に見えるので低頻度（#1）。
    pub discover_interval_minutes: NonZeroU32,
    /// 最後に検知が通った日時。まだ一度も通っていなければ空。
    ///
    /// **通ったときだけ動かす。** 落ちた回に進めると、その回に配信元が返せなかった
    /// ものを二度と見に行かなくなる。
    pub last_discovered_at: Option<Timestamp>,
}

impl Source {
    /// 取得が終わった項目が `holding` を通るかどうか。
    ///
    /// `hold_days` が空なら捨てないので、そのまま `kept` になる（#1）。
    pub const fn hold_policy(&self) -> HoldPolicy {
        match self.hold_days {
            Some(_) => HoldPolicy::Hold,
            None => HoldPolicy::NoHold,
        }
    }

    /// いま検知を回す番か。
    ///
    /// 一度も通っていなければ回す。以後は前回から `discover_interval_minutes`。
    pub fn due(&self, now: Timestamp) -> bool {
        match self.last_discovered_at {
            None => true,
            Some(last) => now >= last + self.interval(),
        }
    }

    /// 検知でどこまで遡るか。[`crate::adapter::Discover::discover`] の `since`。
    ///
    /// 前回の続きから見るが、**1周ぶん重ねる**。配信元は投稿の直後に一覧へ載せると
    /// は限らないので、境界ちょうどで切ると載るのが遅れたものを落とす。重なったぶんは
    /// `Item.url` が一意なので二重に行を作らない。
    ///
    /// `created_at` より前へは行かない。#1 の「登録日時。これ以降の投稿を監視する」。
    pub fn discover_since(&self) -> Timestamp {
        match self.last_discovered_at {
            None => self.created_at,
            Some(last) => (last - self.interval()).max(self.created_at),
        }
    }

    fn interval(&self) -> chrono::TimeDelta {
        // 分は最大 u32 なので、TimeDelta（i64 ミリ秒）に収まる。
        chrono::TimeDelta::minutes(i64::from(self.discover_interval_minutes.get()))
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
            discover_interval_minutes: NonZeroU32::new(15).unwrap(),
            last_discovered_at: None,
        }
    }

    /// #1 の「`hold_days` が空なら捨てない」。
    #[test]
    fn hold_policy_follows_hold_days() {
        assert_eq!(source(Some(7)).hold_policy(), HoldPolicy::Hold);
        assert_eq!(source(None).hold_policy(), HoldPolicy::NoHold);
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

//! `Item` の状態と、その遷移。#1 の stateDiagram をそのまま写す。

use thiserror::Error;

use crate::liveness::PresenceConfirmed;
use crate::timestamp::Timestamp;

/// #1 の状態表。
///
/// 状態と対でしか意味を持たない値は、ここで対にする（#1）。DB では平らなカラム
/// だが、`release_reference` を持てるのは `Released` だけ、`hold_until` を持てるのは
/// `Transcribing` と `Holding` だけ、という対応はこの enum が保証する。
///
/// 状態を組み直すのは、**行を丸ごと見る parse 層**の仕事。`state` の1列では決まらず、
/// `hold_until` と `release_reference` が揃って初めて決まるので、1列だけ見る `FromStr`
/// は黙って `Released { reference: None }` を作ってしまう。名前だけが要る場面には
/// [`StateName`] がある。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// 見つけたが、まだ取得していない。
    Waiting,
    /// 取得中。配信の録画、または VOD のダウンロード。
    Acquiring,
    /// 文字起こし中。終わったら `hold` の指す先へ進む。
    ///
    /// 期限を運ぶのは [`Event::Acquired`] の1回きりで、[`Event::Transcribed`] は
    /// 値を持たない。状態が持っていれば、ログの中で期限が食い違う書き方ができない（#1）。
    Transcribing { hold: Hold },
    /// 預かり中。この期限まで配信元を確認し続ける。
    ///
    /// 期限を伴わない `holding` は作れない。#1 の「期限のない預かりを表現できない」を、
    /// 事象だけでなく状態の側でも成り立たせる。
    Holding { until: Timestamp },
    /// 保持が確定し、引き渡しを待つ。終端ではない。
    Kept,
    /// 期限まで配信元に在ったので捨てた。
    Discarded,
    /// 外部が受け取ったので手放した。
    Released {
        /// 移した先への参照。escrow は解釈せず保管する（#1）。
        reference: Option<ReleaseReference>,
    },
    /// 人が手で消した。
    Deleted,
    /// 取得できなかった。理由は [`Event::AttemptFailed`] が持ち、状態には載せない（#1）。
    Error,
}

/// 状態の名前だけ。
///
/// 値を伴わない場面 — DB の絞り込み、#4 の `--state`、#6 の一覧の見出し — で使う。
/// [`State`] と違って伴う値を持たないので、名前から状態を復元するつもりの誤用が起きない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StateName {
    Waiting,
    Acquiring,
    Transcribing,
    Holding,
    Kept,
    Discarded,
    Released,
    Deleted,
    Error,
}

impl StateName {
    /// #1 の状態表の値。DB と #4 の JSON に出る文字列。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Acquiring => "acquiring",
            Self::Transcribing => "transcribing",
            Self::Holding => "holding",
            Self::Kept => "kept",
            Self::Discarded => "discarded",
            Self::Released => "released",
            Self::Deleted => "deleted",
            Self::Error => "error",
        }
    }

    pub const ALL: [Self; 9] = [
        Self::Waiting,
        Self::Acquiring,
        Self::Transcribing,
        Self::Holding,
        Self::Kept,
        Self::Discarded,
        Self::Released,
        Self::Deleted,
        Self::Error,
    ];

    /// #1 の stateDiagram の `live` 合成状態。ここからは `deleted` へ行ける。
    ///
    /// 伴う値を見ないので、名前の側が決める。[`State::is_live`] はここへ委ねる。
    pub const fn is_live(self) -> bool {
        match self {
            Self::Waiting | Self::Acquiring | Self::Transcribing | Self::Holding | Self::Kept => {
                true
            }
            Self::Discarded | Self::Released | Self::Deleted | Self::Error => false,
        }
    }

    /// 終端。人が再取得を指示しない限り動かない。
    pub const fn is_terminal(self) -> bool {
        !self.is_live()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("escrow が知らない状態: {0}")]
pub struct UnknownState(pub String);

impl std::str::FromStr for StateName {
    type Err = UnknownState;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|n| n.as_str() == s)
            .ok_or_else(|| UnknownState(s.to_owned()))
    }
}

impl std::fmt::Display for StateName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl State {
    /// 伴う値を落とした名前。
    pub const fn name(&self) -> StateName {
        match self {
            Self::Waiting => StateName::Waiting,
            Self::Acquiring => StateName::Acquiring,
            Self::Transcribing { .. } => StateName::Transcribing,
            Self::Holding { .. } => StateName::Holding,
            Self::Kept => StateName::Kept,
            Self::Discarded => StateName::Discarded,
            Self::Released { .. } => StateName::Released,
            Self::Deleted => StateName::Deleted,
            Self::Error => StateName::Error,
        }
    }

    /// #1 の状態表の値。DB と #4 の JSON に出る文字列。
    pub const fn as_str(&self) -> &'static str {
        self.name().as_str()
    }

    /// 預かりの期限。持たない状態では空（#1 の `ITEM.hold_until`）。
    ///
    /// [`Self::release_reference`] と合わせて、投影の列を書く側の全体像になる。
    /// 読み戻す側はこの3つ（名前・期限・参照）を揃えて [`State`] を組み直す。
    pub const fn hold_until(&self) -> Option<Timestamp> {
        match self {
            Self::Transcribing {
                hold: Hold::Until(until),
            }
            | Self::Holding { until } => Some(*until),
            Self::Transcribing { hold: Hold::None }
            | Self::Waiting
            | Self::Acquiring
            | Self::Kept
            | Self::Discarded
            | Self::Released { .. }
            | Self::Deleted
            | Self::Error => None,
        }
    }

    /// 外部が保存した先への参照。`Released` 以外では空（#1 の `ITEM.release_reference`）。
    pub const fn release_reference(&self) -> Option<&ReleaseReference> {
        match self {
            Self::Released { reference } => reference.as_ref(),
            Self::Waiting
            | Self::Acquiring
            | Self::Transcribing { .. }
            | Self::Holding { .. }
            | Self::Kept
            | Self::Discarded
            | Self::Deleted
            | Self::Error => None,
        }
    }

    /// 見つけた直後の状態。#1 の `[*]` から出る2本。
    pub const fn initial(media: MediaPresence) -> Self {
        match media {
            MediaPresence::Present => Self::Waiting,
            MediaPresence::Absent => Self::Kept,
        }
    }

    /// #1 の stateDiagram の `live` 合成状態。ここからは `deleted` へ行ける。
    pub const fn is_live(&self) -> bool {
        self.name().is_live()
    }

    /// 終端。人が再取得を指示しない限り動かない。
    pub const fn is_terminal(&self) -> bool {
        self.name().is_terminal()
    }
}

/// いつまで預かるか。**取得が終わった時点で確定する**（#1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// この日時まで預かる。期限まで配信元に在り続けたら捨てる。
    Until(Timestamp),
    /// 期限を持たない。捨てないので `holding` を通らず、そのまま `kept` になる。
    None,
}

/// 預かる日数が大きすぎて、期限が日時にならないとき。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{days} 日先は暦の外")]
pub struct HoldTooFar {
    pub days: std::num::NonZeroU32,
}

impl Hold {
    /// 日数を1つの期限に変える。**渡す時刻は取得が終わった瞬間**（#1）。
    ///
    /// 数時間の録画では、始めた時刻と終わった時刻がその長さぶんずれる。#1 が
    /// 「取得完了の時点で確定する」と書いているのはこの差のことなので、呼ぶ側は
    /// `acquired` を書く直前の時刻を渡す。
    ///
    /// 空を返さないのは、[`Hold::None`]（期限なし＝捨てない）と、日数が表せる範囲を
    /// 超えたこととが**意味の反転した2つ**だから。片方をもう片方に化けさせない。
    pub fn from_days(
        days: Option<std::num::NonZeroU32>,
        acquired_at: Timestamp,
    ) -> Result<Self, HoldTooFar> {
        let Some(days) = days else {
            return Ok(Self::None);
        };
        acquired_at
            .plus_days(days)
            .map(Self::Until)
            .ok_or(HoldTooFar { days })
    }
}

/// 外部が保存した先への参照。escrow は解釈せず保管する（#1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseReference(String);

impl ReleaseReference {
    pub fn new(reference: impl Into<String>) -> Self {
        Self(reference.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 取得や文字起こしが1回失敗した理由。escrow は解釈せず保管する（#1）。
///
/// 中身は外部ツールの出力だが、**回数を数えるために事象そのものが要る**ので、
/// #1 の「履歴を残す範囲は stateDiagram だけ」の外には出ない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureReason(String);

impl FailureReason {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 取得する実体があるか。テキストだけの X 投稿は `Absent`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaPresence {
    Present,
    Absent,
}

/// 文字起こしする実体があるか。#1 の3つのスイッチの1つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptNeed {
    Needed,
    NotNeeded,
}

/// 状態を動かす出来事。
///
/// 項目の誕生（`discovered`）はここに入らない。状態を**動かす**のではなく**作る**ので、
/// [`next`] の引数にならない。混ぜると、受け皿を置かないこの関数に不正な腕が9本生える。
/// 誕生が運ぶものは [`crate::item::Discovered`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 取得を始めた。
    AcquisitionStarted,
    /// 取得が終わった。次にどこへ行くかは2つのスイッチが決める（#1 の「経路が分かれる理由」）。
    ///
    /// **預かりの期限を運ぶのはこの事象だけ。** 以降は状態が持つ。
    Acquired {
        transcript: TranscriptNeed,
        hold: Hold,
    },
    /// 文字起こしが終わった。行き先は `Transcribing` が持っている期限が決める。
    Transcribed,
    /// 預かり中に配信元から消えた。手元のものを残す。
    SourceGone,
    /// 預かり中に配信元へ在ることを確かめた。**状態は動かない。**
    ///
    /// #5 の「期限が過ぎていても、直近の確認で『在る』が取れていなければ捨てない」に
    /// 居場所を与える。確認できなかった回は行が増えないので、**沈黙が記録されない
    /// ことが、そのまま #5 の非対称性になる**（#1）。
    PresenceConfirmed(PresenceConfirmed),
    /// 期限まで配信元に在ることを確かめた。捨ててよい。
    ///
    /// 証を要求するので、期限が過ぎたというだけで捨てる実装は書けない。
    HeldToDeadline(PresenceConfirmed),
    /// 外部が受け取った。
    Released { reference: Option<ReleaseReference> },
    /// 人が消した。
    Deleted,
    /// 取得か文字起こしが1回失敗した。**状態は動かない。**
    ///
    /// リトライ回数はこの事象を数えて導出する。`RetriesExhausted` は終端なので、
    /// それだけでは最後の1回しか残らず数えられない（#1）。
    AttemptFailed { reason: FailureReason },
    /// リトライ上限に達した。
    RetriesExhausted,
    /// 人が再取得を指示した。自動では起きない（#1）。
    ReacquisitionRequested,
}

/// ログに残る事象の判別子。DB の `item_event.kind`（#1）。
///
/// [`Event`] より1つ多い。`Discovered` は状態を動かさないので [`Event`] には無いが、
/// 保存の形では他と同じ1行になる。**行を読む側はこの enum で全域に分岐し**、
/// `Discovered` を先頭の1件へ、残りを [`Event`] へ振り分ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EventKind {
    Discovered,
    AcquisitionStarted,
    Acquired,
    Transcribed,
    SourceGone,
    PresenceConfirmed,
    HeldToDeadline,
    Released,
    Deleted,
    AttemptFailed,
    RetriesExhausted,
    ReacquisitionRequested,
}

impl EventKind {
    /// DB に入る文字列。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::AcquisitionStarted => "acquisition_started",
            Self::Acquired => "acquired",
            Self::Transcribed => "transcribed",
            Self::SourceGone => "source_gone",
            Self::PresenceConfirmed => "presence_confirmed",
            Self::HeldToDeadline => "held_to_deadline",
            Self::Released => "released",
            Self::Deleted => "deleted",
            Self::AttemptFailed => "attempt_failed",
            Self::RetriesExhausted => "retries_exhausted",
            Self::ReacquisitionRequested => "reacquisition_requested",
        }
    }

    pub const ALL: [Self; 12] = [
        Self::Discovered,
        Self::AcquisitionStarted,
        Self::Acquired,
        Self::Transcribed,
        Self::SourceGone,
        Self::PresenceConfirmed,
        Self::HeldToDeadline,
        Self::Released,
        Self::Deleted,
        Self::AttemptFailed,
        Self::RetriesExhausted,
        Self::ReacquisitionRequested,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("escrow が知らない事象: {0}")]
pub struct UnknownEventKind(pub String);

impl std::str::FromStr for EventKind {
    type Err = UnknownEventKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| UnknownEventKind(s.to_owned()))
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Event {
    /// 保存の形での判別子。[`EventKind::Discovered`] は返らない。
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::AcquisitionStarted => EventKind::AcquisitionStarted,
            Self::Acquired { .. } => EventKind::Acquired,
            Self::Transcribed => EventKind::Transcribed,
            Self::SourceGone => EventKind::SourceGone,
            Self::PresenceConfirmed(_) => EventKind::PresenceConfirmed,
            Self::HeldToDeadline(_) => EventKind::HeldToDeadline,
            Self::Released { .. } => EventKind::Released,
            Self::Deleted => EventKind::Deleted,
            Self::AttemptFailed { .. } => EventKind::AttemptFailed,
            Self::RetriesExhausted => EventKind::RetriesExhausted,
            Self::ReacquisitionRequested => EventKind::ReacquisitionRequested,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{from} には {event} を掛けられない", from = .from.as_str(), event = .event.kind())]
pub struct IllegalTransition {
    pub from: State,
    pub event: Event,
}

/// 状態遷移。#1 の stateDiagram をそのまま写した全域関数。
///
/// 事象ごとに、受け付ける状態を並べる。内側の `match` は 9 状態を全部書き、
/// **受け皿（`_ =>`）を置かない**。状態か事象を足すとここが軒並みコンパイルエラーになるので、
/// 不正な遷移が「たまたま通る」ことがない。
///
/// 事象を保存する形にしたので、この関数がそのまま**ログを畳む関数**になる（#15）。
pub fn next(state: &State, event: &Event) -> Result<State, IllegalTransition> {
    use Event as E;
    use State as S;

    let illegal = || IllegalTransition {
        from: state.clone(),
        event: event.clone(),
    };

    let next = match event {
        E::AcquisitionStarted => match state {
            S::Waiting => S::Acquiring,
            S::Acquiring
            | S::Transcribing { .. }
            | S::Holding { .. }
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::Acquired { transcript, hold } => match state {
            S::Acquiring => match (transcript, hold) {
                (TranscriptNeed::Needed, hold) => S::Transcribing { hold: *hold },
                (TranscriptNeed::NotNeeded, Hold::Until(until)) => S::Holding { until: *until },
                (TranscriptNeed::NotNeeded, Hold::None) => S::Kept,
            },
            S::Waiting
            | S::Transcribing { .. }
            | S::Holding { .. }
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::Transcribed => match state {
            S::Transcribing { hold } => match hold {
                Hold::Until(until) => S::Holding { until: *until },
                Hold::None => S::Kept,
            },
            S::Waiting
            | S::Acquiring
            | S::Holding { .. }
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::SourceGone => match state {
            S::Holding { .. } => S::Kept,
            S::Waiting
            | S::Acquiring
            | S::Transcribing { .. }
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        // 状態を動かさない。確かめたという事実だけが残る（#1）。
        E::PresenceConfirmed(_) => match state {
            S::Holding { until } => S::Holding { until: *until },
            S::Waiting
            | S::Acquiring
            | S::Transcribing { .. }
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::HeldToDeadline(_) => match state {
            S::Holding { .. } => S::Discarded,
            S::Waiting
            | S::Acquiring
            | S::Transcribing { .. }
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        // #4 の「`holding` の項目も `list` に出るが、この場合 `release` は使えない」。
        E::Released { reference } => match state {
            S::Kept => S::Released {
                reference: reference.clone(),
            },
            S::Waiting
            | S::Acquiring
            | S::Transcribing { .. }
            | S::Holding { .. }
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::Deleted => match state {
            S::Waiting | S::Acquiring | S::Transcribing { .. } | S::Holding { .. } | S::Kept => {
                S::Deleted
            }
            S::Discarded | S::Released { .. } | S::Deleted | S::Error => return Err(illegal()),
        },

        // 状態を動かさない。何度でも積み上がり、その本数がリトライ回数になる（#1）。
        E::AttemptFailed { .. } => match state {
            S::Acquiring => S::Acquiring,
            S::Transcribing { hold } => S::Transcribing { hold: *hold },
            S::Waiting
            | S::Holding { .. }
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::RetriesExhausted => match state {
            S::Acquiring | S::Transcribing { .. } => S::Error,
            S::Waiting
            | S::Holding { .. }
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::ReacquisitionRequested => match state {
            S::Discarded | S::Released { .. } | S::Deleted | S::Error => S::Waiting,
            S::Waiting | S::Acquiring | S::Transcribing { .. } | S::Holding { .. } | S::Kept => {
                return Err(illegal());
            }
        },
    };

    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liveness::Presence;

    fn confirmed() -> PresenceConfirmed {
        Presence::Present
            .confirmed()
            .expect("Present なら証が取れる")
    }

    fn deadline() -> Timestamp {
        Timestamp::parse("2026-09-10T22:10:00+09:00").expect("固定値")
    }

    /// 全状態の代表値。網羅の勘定に使う。
    ///
    /// 本体に置かないのは、`Holding` の期限を代表値として捏造することになるため。
    /// 「どの状態も等しく作れる」は勘定のための都合で、要件ではない。
    fn all_states() -> [State; 9] {
        [
            State::Waiting,
            State::Acquiring,
            State::Transcribing { hold: Hold::None },
            State::Holding { until: deadline() },
            State::Kept,
            State::Discarded,
            State::Released { reference: None },
            State::Deleted,
            State::Error,
        ]
    }

    /// 全事象の代表値。網羅の勘定に使う。
    fn all_events() -> [Event; 11] {
        [
            Event::AcquisitionStarted,
            Event::Acquired {
                transcript: TranscriptNeed::Needed,
                hold: Hold::None,
            },
            Event::Transcribed,
            Event::SourceGone,
            Event::PresenceConfirmed(confirmed()),
            Event::HeldToDeadline(confirmed()),
            Event::Released { reference: None },
            Event::Deleted,
            Event::AttemptFailed {
                reason: FailureReason::new("HTTP 403"),
            },
            Event::RetriesExhausted,
            Event::ReacquisitionRequested,
        ]
    }

    /// #1 の stateDiagram に描かれている遷移を全部並べたもの。
    /// 図が動いたらここが落ちる。
    #[test]
    fn reproduces_every_transition_in_the_diagram() {
        let cases: Vec<(State, Event, State)> = vec![
            (State::Waiting, Event::AcquisitionStarted, State::Acquiring),
            // acquiring から出る4本。分岐させるのはプラットフォームではなくスイッチ。
            (
                State::Acquiring,
                Event::Acquired {
                    transcript: TranscriptNeed::Needed,
                    hold: Hold::Until(deadline()),
                },
                State::Transcribing {
                    hold: Hold::Until(deadline()),
                },
            ),
            (
                State::Acquiring,
                Event::Acquired {
                    transcript: TranscriptNeed::Needed,
                    hold: Hold::None,
                },
                State::Transcribing { hold: Hold::None },
            ),
            (
                State::Acquiring,
                Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: Hold::Until(deadline()),
                },
                State::Holding { until: deadline() },
            ),
            (
                State::Acquiring,
                Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: Hold::None,
                },
                State::Kept,
            ),
            // transcribing から出る2本。行き先を決めるのは状態が持っている期限。
            (
                State::Transcribing {
                    hold: Hold::Until(deadline()),
                },
                Event::Transcribed,
                State::Holding { until: deadline() },
            ),
            (
                State::Transcribing { hold: Hold::None },
                Event::Transcribed,
                State::Kept,
            ),
            (
                State::Holding { until: deadline() },
                Event::SourceGone,
                State::Kept,
            ),
            (
                State::Holding { until: deadline() },
                Event::HeldToDeadline(confirmed()),
                State::Discarded,
            ),
            // 状態を動かさない2つ。
            (
                State::Holding { until: deadline() },
                Event::PresenceConfirmed(confirmed()),
                State::Holding { until: deadline() },
            ),
            (
                State::Acquiring,
                Event::AttemptFailed {
                    reason: FailureReason::new("HTTP 403"),
                },
                State::Acquiring,
            ),
            (
                State::Transcribing {
                    hold: Hold::Until(deadline()),
                },
                Event::AttemptFailed {
                    reason: FailureReason::new("whisper が落ちた"),
                },
                State::Transcribing {
                    hold: Hold::Until(deadline()),
                },
            ),
            (
                State::Kept,
                Event::Released { reference: None },
                State::Released { reference: None },
            ),
            // live のどこからでも deleted へ。
            (State::Waiting, Event::Deleted, State::Deleted),
            (State::Acquiring, Event::Deleted, State::Deleted),
            (
                State::Transcribing { hold: Hold::None },
                Event::Deleted,
                State::Deleted,
            ),
            (
                State::Holding { until: deadline() },
                Event::Deleted,
                State::Deleted,
            ),
            (State::Kept, Event::Deleted, State::Deleted),
            // リトライ上限。
            (State::Acquiring, Event::RetriesExhausted, State::Error),
            (
                State::Transcribing { hold: Hold::None },
                Event::RetriesExhausted,
                State::Error,
            ),
            // 終端から、人の指示でだけ戻る。
            (
                State::Discarded,
                Event::ReacquisitionRequested,
                State::Waiting,
            ),
            (
                State::Released { reference: None },
                Event::ReacquisitionRequested,
                State::Waiting,
            ),
            (
                State::Deleted,
                Event::ReacquisitionRequested,
                State::Waiting,
            ),
            (State::Error, Event::ReacquisitionRequested, State::Waiting),
        ];

        for (from, event, expected) in cases {
            let got = next(&from, &event).unwrap_or_else(|e| panic!("図にある遷移が通らない: {e}"));
            assert_eq!(got, expected, "{} + {}", from.as_str(), event.kind());
        }
    }

    /// 図に無い組み合わせはすべて拒まれる。
    ///
    /// 9 状態 × 11 事象 = 99 通りのうち、通るのはちょうど 20 通り。
    /// 遷移を増やすとこの数が動くので、図を書き換えずに実装だけ緩めることができない。
    #[test]
    fn exactly_the_diagram_is_legal() {
        let mut legal = 0;
        let mut total = 0;

        for state in all_states() {
            for event in all_events() {
                total += 1;
                if next(&state, &event).is_ok() {
                    legal += 1;
                }
            }
        }

        assert_eq!(total, 99);
        assert_eq!(legal, 20, "図にある遷移は 20 本");
    }

    /// 期限を運ぶのは `acquired` の1回きり（#1）。
    ///
    /// `Transcribed` が値を持たないので、`acquired` が `Until(X)`、`transcribed` が
    /// `Until(Y)` というログは**書けない**。行き先は状態が持っている期限だけが決める。
    #[test]
    fn the_deadline_travels_in_the_state_not_in_transcribed() {
        let acquired = Event::Acquired {
            transcript: TranscriptNeed::Needed,
            hold: Hold::Until(deadline()),
        };
        let transcribing = next(&State::Acquiring, &acquired).unwrap();
        assert_eq!(transcribing.hold_until(), Some(deadline()));

        let holding = next(&transcribing, &Event::Transcribed).unwrap();
        assert_eq!(holding, State::Holding { until: deadline() });
    }

    /// 期限を伴わない `holding` が作れないこと（#1）。
    ///
    /// `holding` へ入る道は2本しか無く、どちらも期限を持ってしか通れない。
    #[test]
    fn holding_cannot_exist_without_a_deadline() {
        assert_eq!(
            next(
                &State::Transcribing { hold: Hold::None },
                &Event::Transcribed
            )
            .unwrap(),
            State::Kept,
            "期限が無ければ holding を飛ばす"
        );
        assert_eq!(
            next(
                &State::Acquiring,
                &Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: Hold::None,
                }
            )
            .unwrap(),
            State::Kept,
        );
    }

    /// 失敗は積み上がるだけで状態を動かさない（#1）。
    ///
    /// これが無いと `retries_exhausted` の1本しか残らず、リトライ回数を数えられない。
    #[test]
    fn failures_pile_up_without_moving_the_state() {
        let failed = Event::AttemptFailed {
            reason: FailureReason::new("HTTP 403"),
        };

        let mut state = State::Acquiring;
        for _ in 0..3 {
            state = next(&state, &failed).unwrap();
        }
        assert_eq!(state, State::Acquiring);

        // 上限に達して初めて終端へ。
        assert_eq!(
            next(&state, &Event::RetriesExhausted).unwrap(),
            State::Error
        );
    }

    /// 沈黙で捨てられないこと。証は `Presence::Present` からしか出ない。
    #[test]
    fn discarding_needs_a_confirmed_presence() {
        assert!(Presence::Unknown.confirmed().is_none());
        assert!(Presence::Gone.confirmed().is_none());

        // 証があってはじめて事象が作れ、そこではじめて discarded へ行ける。
        let event = Event::HeldToDeadline(Presence::Present.confirmed().unwrap());
        assert_eq!(
            next(&State::Holding { until: deadline() }, &event).unwrap(),
            State::Discarded
        );
    }

    /// #4 の決め事。`holding` は `list` に出るが `release` は使えない。
    #[test]
    fn release_is_only_reachable_from_kept() {
        let event = Event::Released {
            reference: Some(ReleaseReference::new("Attachments/2026-03-01 ○○.mp4")),
        };

        assert!(next(&State::Holding { until: deadline() }, &event).is_err());
        assert!(next(&State::Waiting, &event).is_err());

        let released = next(&State::Kept, &event).unwrap();
        assert_eq!(
            released,
            State::Released {
                reference: Some(ReleaseReference::new("Attachments/2026-03-01 ○○.mp4")),
            }
        );
        assert_eq!(released.as_str(), "released");
    }

    #[test]
    fn initial_state_follows_whether_there_is_media_to_fetch() {
        assert_eq!(State::initial(MediaPresence::Present), State::Waiting);
        assert_eq!(State::initial(MediaPresence::Absent), State::Kept);
    }

    /// 状態と名前が1対1であること。どちらかを足したらここが落ちる。
    #[test]
    fn every_state_has_a_name_and_back() {
        let names: Vec<StateName> = all_states().iter().map(State::name).collect();
        assert_eq!(names, StateName::ALL.to_vec());

        for name in StateName::ALL {
            assert_eq!(name.as_str().parse::<StateName>().unwrap(), name);
        }
        assert!("gone".parse::<StateName>().is_err());
    }

    /// 事象と判別子が1対1であること。
    ///
    /// `Discovered` だけは [`Event`] に居ないので、`kind()` からは出てこない。
    /// 保存の形では他と同じ1行になるので、判別子の側には在る。
    #[test]
    fn every_event_kind_round_trips() {
        for kind in EventKind::ALL {
            assert_eq!(kind.as_str().parse::<EventKind>().unwrap(), kind);
        }
        assert!("archived".parse::<EventKind>().is_err());

        let from_events: Vec<EventKind> = all_events().iter().map(Event::kind).collect();
        let expected: Vec<EventKind> = EventKind::ALL
            .into_iter()
            .filter(|k| *k != EventKind::Discovered)
            .collect();
        assert_eq!(from_events, expected);
    }

    /// 投影へ書く列と、状態が1対1であること。
    ///
    /// 読み戻す側はこの3つ（名前・期限・参照）から状態を組み直すので、
    /// 書く側が落とすものがあると往復しない。
    #[test]
    fn the_projected_columns_cover_what_the_state_carries() {
        for state in all_states() {
            let has_payload = state.hold_until().is_some() || state.release_reference().is_some();
            let carries_one = matches!(
                state,
                State::Transcribing {
                    hold: Hold::Until(_)
                } | State::Holding { .. }
                    | State::Released { reference: Some(_) }
            );
            assert_eq!(has_payload, carries_one, "{}", state.as_str());
        }

        assert_eq!(State::Kept.hold_until(), None);
        assert_eq!(
            State::Transcribing { hold: Hold::None }.hold_until(),
            None,
            "期限を持たない transcribing は列も空"
        );
    }

    #[test]
    fn kept_is_not_terminal() {
        assert!(State::Kept.is_live());
        assert!(!State::Kept.is_terminal());

        for terminal in [
            State::Discarded,
            State::Released { reference: None },
            State::Deleted,
            State::Error,
        ] {
            assert!(terminal.is_terminal(), "{}", terminal.as_str());
        }
    }
}

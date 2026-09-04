//! `Item` の状態と、その遷移。#1 の stateDiagram をそのまま写す。

use thiserror::Error;

use crate::liveness::PresenceConfirmed;

/// #1 の状態表。
///
/// `release_reference` は `Released` だけが持つ。DB では平らなカラムだが、
/// 「状態と対でしか意味を持たない値」を対にするのはここ（#1）。
///
/// 文字列から作る口はここに置かない。`state` と `release_reference` の2列を
/// 揃えて初めて決まるので、片方だけ見る `FromStr` は黙って `Released { reference: None }`
/// を作ってしまう。読み戻しは行を丸ごと見る parse 層の仕事で、名前だけが要る
/// 場面には [`StateName`] がある。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// 見つけたが、まだ取得していない。
    Waiting,
    /// 取得中。配信の録画、または VOD のダウンロード。
    Acquiring,
    /// 文字起こし中。
    Transcribing,
    /// 預かり中。期限まで配信元を確認し続ける。
    Holding,
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
    /// 取得できなかった。理由はログが持ち、ここには載せない（#1）。
    Error,
}

/// 状態の名前だけ。
///
/// 値を伴わない場面 — DB の絞り込み、#4 の `--state`、#6 の一覧の見出し — で使う。
/// [`State`] と違って `release_reference` を持たないので、名前から状態を復元する
/// つもりの誤用が起きない。
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
    /// 値を落とした名前。
    pub const fn name(&self) -> StateName {
        match self {
            Self::Waiting => StateName::Waiting,
            Self::Acquiring => StateName::Acquiring,
            Self::Transcribing => StateName::Transcribing,
            Self::Holding => StateName::Holding,
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

    /// 見つけた直後の状態。#1 の `[*]` から出る2本。
    pub const fn initial(media: MediaPresence) -> Self {
        match media {
            MediaPresence::Present => Self::Waiting,
            MediaPresence::Absent => Self::Kept,
        }
    }

    /// #1 の stateDiagram の `live` 合成状態。ここからは `deleted` へ行ける。
    pub const fn is_live(&self) -> bool {
        match self {
            Self::Waiting | Self::Acquiring | Self::Transcribing | Self::Holding | Self::Kept => {
                true
            }
            Self::Discarded | Self::Released { .. } | Self::Deleted | Self::Error => false,
        }
    }

    /// 終端。人が再取得を指示しない限り動かない。
    pub const fn is_terminal(&self) -> bool {
        !self.is_live()
    }

    /// 全状態の代表値。`Released` の参照は空で作る。
    pub fn all() -> [Self; 9] {
        [
            Self::Waiting,
            Self::Acquiring,
            Self::Transcribing,
            Self::Holding,
            Self::Kept,
            Self::Discarded,
            Self::Released { reference: None },
            Self::Deleted,
            Self::Error,
        ]
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

/// `Source.hold_days` があるか。無ければ捨てないので `holding` を飛ばす（#1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldPolicy {
    Hold,
    NoHold,
}

/// 状態を動かす出来事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 取得を始めた。
    AcquisitionStarted,
    /// 取得が終わった。次にどこへ行くかは2つのスイッチが決める（#1 の「経路が分かれる理由」）。
    Acquired {
        transcript: TranscriptNeed,
        hold: HoldPolicy,
    },
    /// 文字起こしが終わった。
    Transcribed { hold: HoldPolicy },
    /// 預かり中に配信元から消えた。手元のものを残す。
    SourceGone,
    /// 期限まで配信元に在ることを確かめた。捨ててよい。
    ///
    /// 証を要求するので、`state_since + hold_days < now` だけで捨てる実装は書けない。
    HeldToDeadline(PresenceConfirmed),
    /// 外部が受け取った。
    Released { reference: Option<ReleaseReference> },
    /// 人が消した。
    Deleted,
    /// リトライ上限に達した。
    RetriesExhausted,
    /// 人が再取得を指示した。自動では起きない（#1）。
    ReacquisitionRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{from} には {event} を掛けられない", from = .from.as_str(), event = .event.name())]
pub struct IllegalTransition {
    pub from: State,
    pub event: Event,
}

impl Event {
    const fn name(&self) -> &'static str {
        match self {
            Self::AcquisitionStarted => "acquisition_started",
            Self::Acquired { .. } => "acquired",
            Self::Transcribed { .. } => "transcribed",
            Self::SourceGone => "source_gone",
            Self::HeldToDeadline(_) => "held_to_deadline",
            Self::Released { .. } => "released",
            Self::Deleted => "deleted",
            Self::RetriesExhausted => "retries_exhausted",
            Self::ReacquisitionRequested => "reacquisition_requested",
        }
    }
}

/// 状態遷移。#1 の stateDiagram をそのまま写した全域関数。
///
/// 事象ごとに、受け付ける状態を並べる。内側の `match` は 9 状態を全部書き、
/// **受け皿（`_ =>`）を置かない**。状態か事象を足すとここが軒並みコンパイルエラーになるので、
/// 不正な遷移が「たまたま通る」ことがない。
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
            | S::Transcribing
            | S::Holding
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::Acquired { transcript, hold } => match state {
            S::Acquiring => match (transcript, hold) {
                (TranscriptNeed::Needed, HoldPolicy::Hold | HoldPolicy::NoHold) => S::Transcribing,
                (TranscriptNeed::NotNeeded, HoldPolicy::Hold) => S::Holding,
                (TranscriptNeed::NotNeeded, HoldPolicy::NoHold) => S::Kept,
            },
            S::Waiting
            | S::Transcribing
            | S::Holding
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::Transcribed { hold } => match state {
            S::Transcribing => match hold {
                HoldPolicy::Hold => S::Holding,
                HoldPolicy::NoHold => S::Kept,
            },
            S::Waiting
            | S::Acquiring
            | S::Holding
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::SourceGone => match state {
            S::Holding => S::Kept,
            S::Waiting
            | S::Acquiring
            | S::Transcribing
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::HeldToDeadline(_) => match state {
            S::Holding => S::Discarded,
            S::Waiting
            | S::Acquiring
            | S::Transcribing
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
            | S::Transcribing
            | S::Holding
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::Deleted => match state {
            S::Waiting | S::Acquiring | S::Transcribing | S::Holding | S::Kept => S::Deleted,
            S::Discarded | S::Released { .. } | S::Deleted | S::Error => return Err(illegal()),
        },

        E::RetriesExhausted => match state {
            S::Acquiring | S::Transcribing => S::Error,
            S::Waiting
            | S::Holding
            | S::Kept
            | S::Discarded
            | S::Released { .. }
            | S::Deleted
            | S::Error => return Err(illegal()),
        },

        E::ReacquisitionRequested => match state {
            S::Discarded | S::Released { .. } | S::Deleted | S::Error => S::Waiting,
            S::Waiting | S::Acquiring | S::Transcribing | S::Holding | S::Kept => {
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

    /// 全事象の代表値。網羅の勘定に使う。
    fn all_events() -> [Event; 9] {
        [
            Event::AcquisitionStarted,
            Event::Acquired {
                transcript: TranscriptNeed::Needed,
                hold: HoldPolicy::NoHold,
            },
            Event::Transcribed {
                hold: HoldPolicy::NoHold,
            },
            Event::SourceGone,
            Event::HeldToDeadline(confirmed()),
            Event::Released { reference: None },
            Event::Deleted,
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
            // acquiring から出る3本。分岐させるのはプラットフォームではなくスイッチ。
            (
                State::Acquiring,
                Event::Acquired {
                    transcript: TranscriptNeed::Needed,
                    hold: HoldPolicy::Hold,
                },
                State::Transcribing,
            ),
            (
                State::Acquiring,
                Event::Acquired {
                    transcript: TranscriptNeed::Needed,
                    hold: HoldPolicy::NoHold,
                },
                State::Transcribing,
            ),
            (
                State::Acquiring,
                Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: HoldPolicy::Hold,
                },
                State::Holding,
            ),
            (
                State::Acquiring,
                Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: HoldPolicy::NoHold,
                },
                State::Kept,
            ),
            // transcribing から出る2本。
            (
                State::Transcribing,
                Event::Transcribed {
                    hold: HoldPolicy::Hold,
                },
                State::Holding,
            ),
            (
                State::Transcribing,
                Event::Transcribed {
                    hold: HoldPolicy::NoHold,
                },
                State::Kept,
            ),
            (State::Holding, Event::SourceGone, State::Kept),
            (
                State::Holding,
                Event::HeldToDeadline(confirmed()),
                State::Discarded,
            ),
            (
                State::Kept,
                Event::Released { reference: None },
                State::Released { reference: None },
            ),
            // live のどこからでも deleted へ。
            (State::Waiting, Event::Deleted, State::Deleted),
            (State::Acquiring, Event::Deleted, State::Deleted),
            (State::Transcribing, Event::Deleted, State::Deleted),
            (State::Holding, Event::Deleted, State::Deleted),
            (State::Kept, Event::Deleted, State::Deleted),
            // リトライ上限。
            (State::Acquiring, Event::RetriesExhausted, State::Error),
            (State::Transcribing, Event::RetriesExhausted, State::Error),
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
            assert_eq!(got, expected, "{} + {}", from.as_str(), event.name());
        }
    }

    /// 図に無い組み合わせはすべて拒まれる。
    ///
    /// 9 状態 × 9 事象 = 81 通りのうち、通るのはちょうど 17 通り。
    /// 遷移を増やすとこの数が動くので、図を書き換えずに実装だけ緩めることができない。
    #[test]
    fn exactly_the_diagram_is_legal() {
        let mut legal = 0;
        let mut total = 0;

        for state in State::all() {
            for event in all_events() {
                total += 1;
                if next(&state, &event).is_ok() {
                    legal += 1;
                }
            }
        }

        assert_eq!(total, 81);
        assert_eq!(legal, 17, "図にある遷移は 17 本");
    }

    /// 沈黙で捨てられないこと。証は `Presence::Present` からしか出ない。
    #[test]
    fn discarding_needs_a_confirmed_presence() {
        assert!(Presence::Unknown.confirmed().is_none());
        assert!(Presence::Gone.confirmed().is_none());

        // 証があってはじめて事象が作れ、そこではじめて discarded へ行ける。
        let event = Event::HeldToDeadline(Presence::Present.confirmed().unwrap());
        assert_eq!(next(&State::Holding, &event).unwrap(), State::Discarded);
    }

    /// #4 の決め事。`holding` は `list` に出るが `release` は使えない。
    #[test]
    fn release_is_only_reachable_from_kept() {
        let event = Event::Released {
            reference: Some(ReleaseReference::new("Attachments/2026-03-01 ○○.mp4")),
        };

        assert!(next(&State::Holding, &event).is_err());
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
        let names: Vec<StateName> = State::all().iter().map(State::name).collect();
        assert_eq!(names, StateName::ALL.to_vec());

        for name in StateName::ALL {
            assert_eq!(name.as_str().parse::<StateName>().unwrap(), name);
        }
        assert!("gone".parse::<StateName>().is_err());
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

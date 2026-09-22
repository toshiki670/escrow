//! 項目のイベントと、そのリードモデル（#15）。
//!
//! 誕生（[`EventStore::discover`]）と追記（[`EventStore::append`]）と、そこから導ける
//! リードモデル。

use escrow_domain::item::{Discovered, Item, ItemId};
use escrow_domain::state::{Event, IllegalTransition, next};
use escrow_domain::timestamp::Timestamp;

use crate::Seq;

mod append;
mod read_model;
mod replay;

pub(crate) use read_model::Columns;
pub(crate) use replay::{EventRow, log_of};

/// リードモデルから読んだ1件と、その項目の最後のイベントの番号。
///
/// 次のイベントを書くときにこの `seq` を渡すので、**読んでから書くまでの間に誰かが
/// 動かしていれば [`append`](crate::EventStore::append) が
/// [`Superseded`](crate::EventStoreError::Superseded) で弾く**。
/// 読み出しが必ず番号を一緒に返すので、根拠を持たずに書く経路がそもそも作れない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadModelRow {
    pub item: Item,
    pub seq: Seq,
}

/// ログの1行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub seq: Seq,
    pub occurred_at: Timestamp,
    pub event: Event,
}

/// 1つの項目のログ全体。
///
/// **先頭は誕生で、ちょうど1つ。** 型がそう言っているので、`replay` の中で
/// 「先頭が `discovered` か」を確かめる必要が無い。確かめるのは行を読む側の
/// 仕事で、境界はそこ1か所（#1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    pub id: ItemId,
    pub discovered: Discovered,
    pub discovered_at: Timestamp,
    /// 誕生のあとに続くイベント。`seq` の順。
    pub rest: Vec<Recorded>,
}

impl Log {
    /// ログをリプレイして、いまの `Item` を作る。
    ///
    /// リプレイの1歩は #1 の状態遷移そのもの。イベントを保存する形にしたので、手で書いた
    /// 全域関数がそのままここで使える（#15）。
    ///
    /// `state_since` は**状態が変わった**イベントの時刻だけを取る。生存確認や1回の
    /// 失敗を書いても、`holding` になった日時は動かない。
    pub fn replay(&self) -> Result<Item, IllegalTransition> {
        let mut state = self.discovered.initial_state();
        let mut state_since = self.discovered_at;

        for recorded in &self.rest {
            let moved = next(&state, &recorded.event)?;
            if moved != state {
                state_since = recorded.occurred_at;
            }
            state = moved;
        }

        Ok(Item {
            id: self.id,
            source_id: self.discovered.source_id,
            url: self.discovered.url.clone(),
            published_at: self.discovered.published_at,
            scheduled_start_at: self.discovered.scheduled_start_at,
            state,
            state_since,
            content: self.discovered.content.clone(),
        })
    }

    /// 直近の「状態を変えたイベント」より後ろの失敗の本数。
    ///
    /// #1 の「リトライ回数そのものは数えず、イベントから導出する」。イベントから導くだけ
    /// なので、実際と常に一致する。
    pub fn failures_since_the_state_moved(&self) -> Result<usize, IllegalTransition> {
        let mut state = self.discovered.initial_state();
        let mut failures = 0;

        for recorded in &self.rest {
            let moved = next(&state, &recorded.event)?;
            if moved == state {
                if matches!(recorded.event, Event::AttemptFailed { .. }) {
                    failures += 1;
                }
            } else {
                failures = 0;
            }
            state = moved;
        }

        Ok(failures)
    }
}

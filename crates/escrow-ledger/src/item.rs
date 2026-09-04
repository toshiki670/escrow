//! 項目の事象と、その投影（#15）。
//!
//! 書く道は2つだけ — 誕生（[`Ledger::discover`]）と追記（[`Ledger::append`]）。
//! どちらも事象を書いてから、同じトランザクションで投影へ反映する。投影の値は
//! 決定ではなく**決定の写し**なので、写しだけを動かす関数は置かない。

use escrow_domain::item::{Discovered, Item, ItemId};
use escrow_domain::state::{Event, IllegalTransition, next};
use escrow_domain::timestamp::Timestamp;

use crate::Seq;

mod append;
mod projection;
mod replay;

pub(crate) use projection::Columns;
pub(crate) use replay::{EventRow, log_of};

/// 投影から読んだ1件と、その姿を決めた最後の事象の番号。
///
/// 次の事象を書くときにこの `seq` を渡すので、**読んでから書くまでの間に誰かが
/// 動かしていれば弾かれる**。読み出しが必ず番号を連れてくるので、根拠を持たずに
/// 書く経路がそもそも作れない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projected {
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
    /// 誕生のあとに続く事象。`seq` の順。
    pub rest: Vec<Recorded>,
}

impl Log {
    /// ログを畳んで、いまの姿を作る。
    ///
    /// 畳む関数は #1 の状態遷移そのもの。事象を保存する形にしたので、手で書いた
    /// 全域関数がそのままここで使える（#15）。
    ///
    /// `state_since` は**状態が変わった**事象の時刻だけを取る。生存確認や1回の
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

    /// 直近の「状態を変えた事象」より後ろの失敗の本数。
    ///
    /// #1 の「リトライ回数そのものは数えず、事象から導出する」。カウンタを持たない
    /// ので、書き忘れて実際とずれることが起きない。
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

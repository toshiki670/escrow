//! 予算と順番（#13）。
//!
//! 経路ごとに門を1つ持ち、外へ出る要求はそこで順番を待つ。**数えるものは経路で違う** —
//! 頻度で測る経路は最後に出した時刻を見て、取得は走っている数を見る。
//!
//! # 時計が2つある
//!
//! 予算と待ち時間は**経過時間**なので [`tokio::time::Instant`] で測る。テストは
//! `tokio::time::pause` で止めて進められる。一方 [`Plan`] が答える時刻は人が読む
//! **壁の時計**なので、呼ぶ側が渡した `now` に残りの時間を足して作る。

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use escrow_config::{Limits, Schedule};
use escrow_domain::content::Platform;
use escrow_domain::timestamp::Timestamp;
use escrow_external::{AdapterError, Admit, BoxFuture, Permit, Route};

/// 要求に添える、順番を決めるためのもの（#13 の語彙）。
///
/// 要求する側が知っているのはこの2つだけで、上限の値も待ち方も知らない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demand {
    /// いつまでに要るか。**近いものから先に通り、持たないものは最後に回る。**
    ///
    /// 予約枠の開始時刻も、監視期間の終わりも、預かりの期限も、どれもここへ書ける。
    /// 「予約が重みより先に通る」はこの順番から出る（#13）。
    pub deadline: Option<Timestamp>,
    /// 締切が並んだときの順番。`Source.priority` がそのまま入る（#1）。
    pub weight: NonZeroU32,
}

impl Demand {
    /// 締切を持たない要求。空いた分を重みで分け合う。
    pub const fn weighed(weight: NonZeroU32) -> Self {
        Self {
            deadline: None,
            weight,
        }
    }

    /// 締切を持つ要求。
    pub const fn by(deadline: Timestamp, weight: NonZeroU32) -> Self {
        Self {
            deadline: Some(deadline),
            weight,
        }
    }

    /// 人が待っている要求。
    ///
    /// #13 の「対話的なものを最優先にする」。締切をいまに置けば、締切を持たない巡回の
    /// どれよりも、先の締切のどれよりも先に通る。重みは締切が並んだときだけ効くので、
    /// 一番大きい値にしておく。
    pub const fn interactive(now: Timestamp) -> Self {
        Self {
            deadline: Some(now),
            weight: NonZeroU32::MAX,
        }
    }
}

/// いつ出すつもりか（#13 の語彙）。
///
/// **答えられるのは見込みだけ。** 取り置きを持たないので、拒否や待機で後ろへずれる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub platform: Platform,
    pub route: Route,
    pub next: Next,
    /// 順番を待っている要求の数。
    pub waiting: usize,
}

/// 次の要求を出せるのはいつか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// いま出せる。
    Now,
    /// この時刻まで待つ。間隔で測る経路と、拒否で閉じている経路がこれ。
    At(Timestamp),
    /// 走っているものが1つ終わるまで。**終わる時刻は分からない** — 取得は数時間に
    /// 達することがあり、進み具合を外から読む手段が無い。
    WhenOneFinishes,
}

/// 予算の測り方。**経路が決める**（#13）。
#[derive(Debug, Clone, Copy)]
enum Measure {
    /// 要求と要求の間を空ける。頻度で測る経路。
    Gap(Duration),
    /// 同時に走らせる数で測る。長時間・大容量なので回数では測れない。
    Concurrency(NonZeroU32),
}

/// 順番を待っている1件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ticket {
    deadline: Option<Timestamp>,
    weight: NonZeroU32,
    /// 並んだ順。締切と重みが同じものを先着順に並べ、同じ札を作らせない。
    seq: u64,
}

impl Ord for Ticket {
    /// 締切が近いものが先。締切を持たないものは最後。同着なら重みの大きいほうが先で、
    /// それも同じなら先に並んだほうが先。
    fn cmp(&self, other: &Self) -> Ordering {
        let by_deadline = match (self.deadline, other.deadline) {
            (Some(mine), Some(theirs)) => mine.cmp(&theirs),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };

        by_deadline
            .then_with(|| other.weight.cmp(&self.weight))
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

impl PartialOrd for Ticket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 1つの経路の門。
struct Gate {
    measure: Measure,
    /// 拒否されて `Retry-After` が返らなかったときの、最初の待ち時間。
    backoff_base: Duration,
    backoff_max: Duration,
    state: Mutex<GateState>,
    /// 門の状態が動いたことを待っている側へ知らせる。
    ready: Notify,
}

struct GateState {
    waiting: BTreeSet<Ticket>,
    next_seq: u64,
    /// 最後に要求を出した時刻。[`Measure::Gap`] が見る。
    started_at: Option<Instant>,
    /// いま走っている数。[`Measure::Concurrency`] が見る。
    running: u32,
    /// 拒否を受けて閉じている、その終わり。
    closed_until: Option<Instant>,
    /// 次に断られたときに待つ時間。連続で倍になる。
    backoff: Duration,
}

impl Gate {
    fn new(measure: Measure, backoff_base: Duration, backoff_max: Duration) -> Self {
        Self {
            measure,
            backoff_base,
            backoff_max,
            state: Mutex::new(GateState {
                waiting: BTreeSet::new(),
                next_seq: 0,
                started_at: None,
                running: 0,
                closed_until: None,
                backoff: backoff_base,
            }),
            ready: Notify::new(),
        }
    }

    /// 錠が毒されるのは、錠の中で panic したときだけ。中の操作はどれも panic しない。
    fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().expect("門の状態の錠")
    }

    /// 順番が来るまで待つ。
    async fn admit(&self, demand: Demand) -> Slot<'_> {
        let mut queued = self.enqueue(demand);

        loop {
            // 状態を見る前に知らせを受け取れる形にする。見てから待つまでの間に
            // 届いた知らせを取り落とさないため。
            let notified = self.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            match self.take(queued.ticket) {
                Ok(()) => {
                    queued.taken = true;
                    self.ready.notify_waiters();
                    return Slot { gate: self };
                }
                Err(Some(wait)) => {
                    tokio::select! {
                        () = &mut notified => {}
                        () = tokio::time::sleep(wait) => {}
                    }
                }
                Err(None) => notified.await,
            }
        }
    }

    fn enqueue(&self, demand: Demand) -> Queued<'_> {
        let ticket = {
            let mut state = self.lock();
            let ticket = Ticket {
                deadline: demand.deadline,
                weight: demand.weight,
                seq: state.next_seq,
            };
            state.next_seq += 1;
            state.waiting.insert(ticket);
            ticket
        };
        // 並んだことで先頭が入れ替わりうる。待っている側に見直させる。
        self.ready.notify_waiters();

        Queued {
            gate: self,
            ticket,
            taken: false,
        }
    }

    /// 枠を取れたら `Ok`。取れないときは、待つ時間が分かるなら添えて返す。
    fn take(&self, ticket: Ticket) -> Result<(), Option<Duration>> {
        let now = Instant::now();
        let mut state = self.lock();

        if let Some(until) = state.closed_until {
            if until > now {
                return Err(Some(until - now));
            }
            state.closed_until = None;
        }

        if state.waiting.first() != Some(&ticket) {
            return Err(None);
        }

        match self.measure {
            Measure::Gap(gap) => {
                if let Some(started) = state.started_at {
                    let elapsed = now.saturating_duration_since(started);
                    if elapsed < gap {
                        return Err(Some(gap - elapsed));
                    }
                }
                state.started_at = Some(now);
            }
            Measure::Concurrency(limit) => {
                if state.running >= limit.get() {
                    return Err(None);
                }
                state.running += 1;
            }
        }

        state.waiting.remove(&ticket);
        Ok(())
    }

    /// 断られた。この経路を待たせ、次に備えて待ち時間を倍にする（#13）。
    fn rejected(&self, retry_after: Option<Duration>) {
        let mut state = self.lock();
        let wait = retry_after.unwrap_or(state.backoff);

        state.closed_until = Some(Instant::now() + wait);
        state.backoff = state.backoff.saturating_mul(2).min(self.backoff_max);
        drop(state);

        self.ready.notify_waiters();
    }

    /// 通った。次に断られたときの待ち時間を元へ戻す。
    fn passed(&self) {
        self.lock().backoff = self.backoff_base;
    }

    fn plan(&self, platform: Platform, route: Route, now: Timestamp) -> Plan {
        let at = Instant::now();
        let state = self.lock();

        let next = match self.opens(&state, at) {
            Opening::Now => Next::Now,
            Opening::In(wait) => now.plus(wait).map_or(Next::WhenOneFinishes, Next::At),
            Opening::WhenOneFinishes => Next::WhenOneFinishes,
        };

        Plan {
            platform,
            route,
            next,
            waiting: state.waiting.len(),
        }
    }

    fn opens(&self, state: &GateState, now: Instant) -> Opening {
        if let Some(until) = state.closed_until
            && until > now
        {
            return Opening::In(until - now);
        }

        match self.measure {
            Measure::Gap(gap) => match state.started_at {
                Some(started) if now.saturating_duration_since(started) < gap => {
                    Opening::In(gap - now.saturating_duration_since(started))
                }
                _ => Opening::Now,
            },
            Measure::Concurrency(limit) if state.running >= limit.get() => Opening::WhenOneFinishes,
            Measure::Concurrency(_) => Opening::Now,
        }
    }
}

/// 門が開くまで。
enum Opening {
    Now,
    In(Duration),
    WhenOneFinishes,
}

/// 列に並んでいる間の札。**取らずに落ちたら列から抜ける。**
///
/// 呼ぶ側の future が捨てられたときに札が残ると、その門は永久に先頭が動かない。
struct Queued<'a> {
    gate: &'a Gate,
    ticket: Ticket,
    taken: bool,
}

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        if self.taken {
            return;
        }
        self.gate.lock().waiting.remove(&self.ticket);
        self.gate.ready.notify_waiters();
    }
}

/// 順番が回ってきたことの証。**落とすと枠が空く。**
struct Slot<'a> {
    gate: &'a Gate,
}

impl Permit for Slot<'_> {
    fn report(&self, result: Result<(), &AdapterError>) {
        match result {
            Ok(()) => self.gate.passed(),
            Err(AdapterError::Rejected { retry_after, .. }) => self.gate.rejected(*retry_after),
            // 断られた以外の失敗では待ち時間を戻さない。抑えられている最中の失敗で
            // 戻すと、次の拒否が最初の待ち時間からやり直しになる。
            Err(_) => {}
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        if matches!(self.gate.measure, Measure::Concurrency(_)) {
            state.running = state.running.saturating_sub(1);
        }
        drop(state);

        self.gate.ready.notify_waiters();
    }
}

/// 1つのプラットフォームの門。#13 の経路がそのまま並ぶ。
struct Gates {
    discover: Gate,
    describe: Gate,
    acquire: Gate,
    probe: Gate,
}

impl Gates {
    fn new(limits: &Limits, backoff_base: Duration, backoff_max: Duration) -> Self {
        let gap = |seconds: NonZeroU32| {
            Gate::new(
                Measure::Gap(Duration::from_secs(u64::from(seconds.get()))),
                backoff_base,
                backoff_max,
            )
        };

        Self {
            discover: gap(limits.discover_gap_seconds),
            describe: gap(limits.describe_gap_seconds),
            probe: gap(limits.probe_gap_seconds),
            acquire: Gate::new(
                Measure::Concurrency(limits.concurrent_acquisitions),
                backoff_base,
                backoff_max,
            ),
        }
    }

    const fn of(&self, route: Route) -> &Gate {
        match route {
            Route::Discover => &self.discover,
            Route::Describe => &self.describe,
            Route::Acquire => &self.acquire,
            Route::Probe => &self.probe,
        }
    }
}

/// 外部アクセスの総量。**要求を出す側の外に置く**（#13）。
///
/// プラットフォームごとに分けるのは、片方の上限がもう片方を縛らないため。
pub struct Budget {
    youtube: Gates,
    x: Gates,
}

impl Budget {
    pub fn new(schedule: &Schedule) -> Self {
        let seconds = |value: NonZeroU32| Duration::from_secs(u64::from(value.get()));
        let base = seconds(schedule.rejection_backoff_seconds);
        let max = seconds(schedule.rejection_max_backoff_seconds);

        Self {
            youtube: Gates::new(&schedule.youtube, base, max),
            x: Gates::new(&schedule.x, base, max),
        }
    }

    /// 1つの要求ぶんの順番待ち。
    pub const fn turn(&self, platform: Platform, demand: Demand) -> Turn<'_> {
        Turn {
            gates: self.of(platform),
            demand,
        }
    }

    /// いつ出すつもりかを、経路ごとに答える。
    ///
    /// `now` は答えを壁の時計へ写すための起点。門が測っているのは経過時間なので、
    /// 残りをこの時刻へ足して返す。
    pub fn plan(&self, now: Timestamp) -> Vec<Plan> {
        let mut plans = Vec::new();

        for platform in Platform::ALL {
            let gates = self.of(platform);
            for route in Route::ALL {
                plans.push(gates.of(route).plan(platform, route, now));
            }
        }
        plans
    }

    const fn of(&self, platform: Platform) -> &Gates {
        match platform {
            Platform::Youtube => &self.youtube,
            Platform::X => &self.x,
        }
    }
}

/// 1つの要求ぶんの順番待ち。
///
/// **`escrow-external` へ渡すのはこれ。** 締切と重みはここが持ち、向こうは
/// 「この経路の順番を待つ」しか言えない（#13 の疎結合）。
pub struct Turn<'a> {
    gates: &'a Gates,
    demand: Demand,
}

impl Admit for Turn<'_> {
    fn admit(&self, route: Route) -> BoxFuture<'_, Box<dyn Permit + '_>> {
        Box::pin(async move {
            let slot = self.gates.of(route).admit(self.demand).await;
            Box::new(slot) as Box<dyn Permit + '_>
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    fn weight(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).expect("重みは 0 ではない")
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    /// 拒否の待ち時間を 60 秒から始め、1時間で頭打ちにする門。#2 の既定と同じ。
    fn gate(measure: Measure) -> Gate {
        Gate::new(measure, Duration::from_secs(60), Duration::from_secs(3600))
    }

    fn rejected(retry_after: Option<Duration>) -> AdapterError {
        AdapterError::Rejected {
            program: "yt-dlp".to_owned(),
            detail: "429".to_owned(),
            retry_after,
        }
    }

    /// 順番待ちを終えた時刻。
    async fn passes(gate: &Gate, demand: Demand) -> Instant {
        let _slot = gate.admit(demand).await;
        Instant::now()
    }

    /// 間隔で測る経路では、2本目が間隔ぶん待たされる（#7 Phase 5 の受け入れ）。
    #[tokio::test(start_paused = true)]
    async fn a_second_request_waits_out_the_gap() {
        let gate = gate(Measure::Gap(Duration::from_secs(900)));
        let start = Instant::now();

        drop(gate.admit(Demand::weighed(weight(1))).await);
        assert_eq!(Instant::now() - start, Duration::ZERO, "1本目は待たない");

        drop(gate.admit(Demand::weighed(weight(1))).await);
        assert_eq!(Instant::now() - start, Duration::from_secs(900));
    }

    /// 締切を持つ要求が、重みだけの要求より先に通る（#7 Phase 5 の受け入れ）。
    ///
    /// 3本を同じ門で待たせ、通った時刻で順番を見る。締切 → 重みの大きいほう →
    /// 残り、の順に 60 秒ずつずれる。
    #[tokio::test(start_paused = true)]
    async fn a_deadline_goes_before_weight() {
        let gate = gate(Measure::Gap(Duration::from_secs(60)));

        // 門を1回使い、次の 60 秒を閉じる。閉じていないと、先に並んだものが
        // そのまま通ってしまい順番が見えない。
        drop(gate.admit(Demand::weighed(weight(1))).await);
        let start = Instant::now();

        let (heavy, urgent, light) = tokio::join!(
            passes(&gate, Demand::weighed(weight(9))),
            passes(
                &gate,
                Demand::by(at("2026-09-05T12:00:00+09:00"), weight(1))
            ),
            passes(&gate, Demand::weighed(weight(1))),
        );

        assert_eq!(urgent - start, Duration::from_secs(60), "締切が先");
        assert_eq!(heavy - start, Duration::from_secs(120), "次は重いほう");
        assert_eq!(light - start, Duration::from_secs(180));
    }

    /// 締切どうしは近いほうが先。
    #[tokio::test(start_paused = true)]
    async fn the_nearer_deadline_goes_first() {
        let gate = gate(Measure::Gap(Duration::from_secs(60)));
        drop(gate.admit(Demand::weighed(weight(1))).await);
        let start = Instant::now();

        let (far, near) = tokio::join!(
            passes(
                &gate,
                Demand::by(at("2026-09-05T18:00:00+09:00"), weight(9))
            ),
            passes(
                &gate,
                Demand::by(at("2026-09-05T12:00:00+09:00"), weight(1))
            ),
        );

        assert_eq!(near - start, Duration::from_secs(60));
        assert_eq!(far - start, Duration::from_secs(120));
    }

    /// 取得は数で測る。上限まで同時に走り、空くまで次が待つ。
    #[tokio::test(start_paused = true)]
    async fn downloads_run_up_to_the_limit_and_then_wait() {
        let gate = gate(Measure::Concurrency(weight(2)));

        let first = gate.admit(Demand::weighed(weight(1))).await;
        let start = Instant::now();
        let second = gate.admit(Demand::weighed(weight(1))).await;
        assert_eq!(Instant::now() - start, Duration::ZERO, "上限までは待たない");

        let third = passes(&gate, Demand::weighed(weight(1)));
        let finish = async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(first);
        };
        let (third, ()) = tokio::join!(third, finish);

        assert_eq!(third - start, Duration::from_secs(30), "1本空いたら通る");
        drop(second);
    }

    /// 断られたら、その経路をしばらく閉じる（#7 Phase 5 の受け入れ）。
    #[tokio::test(start_paused = true)]
    async fn a_rejection_shuts_the_route_instead_of_hammering_it() {
        // 間隔は 1 秒。**待ちの出どころが拒否だけになる**ようにする。
        let gate = gate(Measure::Gap(Duration::from_secs(1)));

        let slot = gate.admit(Demand::weighed(weight(1))).await;
        slot.report(Err(&rejected(None)));
        drop(slot);

        let start = Instant::now();
        drop(gate.admit(Demand::weighed(weight(1))).await);
        assert_eq!(Instant::now() - start, Duration::from_secs(60), "#2 の既定");
    }

    /// 相手が待ち時間を指定したら、そちらに従う。
    #[tokio::test(start_paused = true)]
    async fn a_stated_retry_after_wins_over_the_default() {
        let gate = gate(Measure::Gap(Duration::from_secs(1)));

        let slot = gate.admit(Demand::weighed(weight(1))).await;
        slot.report(Err(&rejected(Some(Duration::from_secs(5)))));
        drop(slot);

        let start = Instant::now();
        drop(gate.admit(Demand::weighed(weight(1))).await);
        assert_eq!(Instant::now() - start, Duration::from_secs(5));
    }

    /// 連続で断られると待ち時間が倍になり、上限で止まる。通れば元へ戻る。
    #[tokio::test(start_paused = true)]
    async fn the_wait_doubles_while_rejections_continue() {
        let gate = gate(Measure::Gap(Duration::from_secs(1)));
        let mut waits = Vec::new();

        for _ in 0..3 {
            let start = Instant::now();
            let slot = gate.admit(Demand::weighed(weight(1))).await;
            waits.push(Instant::now() - start);
            slot.report(Err(&rejected(None)));
            drop(slot);
        }

        assert_eq!(
            waits,
            vec![
                Duration::ZERO,
                Duration::from_secs(60),
                Duration::from_secs(120)
            ]
        );

        // 通ると次の拒否は最初の待ち時間からやり直す。
        let start = Instant::now();
        let slot = gate.admit(Demand::weighed(weight(1))).await;
        assert_eq!(Instant::now() - start, Duration::from_secs(240));
        slot.report(Ok(()));
        slot.report(Err(&rejected(None)));
        drop(slot);

        let start = Instant::now();
        drop(gate.admit(Demand::weighed(weight(1))).await);
        assert_eq!(Instant::now() - start, Duration::from_secs(60));
    }

    /// 待ち時間は上限で頭打ちになる。
    #[tokio::test(start_paused = true)]
    async fn the_wait_stops_growing_at_the_ceiling() {
        let gate = Gate::new(
            Measure::Gap(Duration::from_secs(1)),
            Duration::from_secs(60),
            Duration::from_secs(120),
        );

        for _ in 0..5 {
            let slot = gate.admit(Demand::weighed(weight(1))).await;
            slot.report(Err(&rejected(None)));
            drop(slot);
        }

        let start = Instant::now();
        drop(gate.admit(Demand::weighed(weight(1))).await);
        assert_eq!(Instant::now() - start, Duration::from_secs(120));
    }

    /// 断られた以外の失敗では、待ち時間を元へ戻さない。
    #[tokio::test(start_paused = true)]
    async fn an_ordinary_failure_leaves_the_wait_where_it_is() {
        let gate = gate(Measure::Gap(Duration::from_secs(1)));

        let slot = gate.admit(Demand::weighed(weight(1))).await;
        slot.report(Err(&rejected(None)));
        drop(slot);

        let slot = gate.admit(Demand::weighed(weight(1))).await;
        slot.report(Err(&AdapterError::Transient {
            program: "yt-dlp".to_owned(),
            detail: "落ちた".to_owned(),
        }));
        slot.report(Err(&rejected(None)));
        drop(slot);

        let start = Instant::now();
        drop(gate.admit(Demand::weighed(weight(1))).await);
        assert_eq!(Instant::now() - start, Duration::from_secs(120), "倍のまま");
    }

    /// 諦めた要求は列から抜ける。残ると、その門は先頭が動かなくなる。
    #[tokio::test(start_paused = true)]
    async fn a_waiter_that_gives_up_leaves_the_queue() {
        let gate = Arc::new(gate(Measure::Concurrency(weight(1))));
        let held = gate.admit(Demand::weighed(weight(1))).await;

        // 締切を持つので列の先頭に立つ。10 秒で諦める。
        let abandoned = tokio::time::timeout(
            Duration::from_secs(10),
            gate.admit(Demand::by(at("2026-09-05T12:00:00+09:00"), weight(9))),
        )
        .await;
        assert!(abandoned.is_err(), "諦めた");
        assert_eq!(gate.lock().waiting.len(), 0, "列に残っていない");

        drop(held);
        let start = Instant::now();
        drop(gate.admit(Demand::weighed(weight(1))).await);
        assert_eq!(Instant::now() - start, Duration::ZERO);
    }

    /// 予定は壁の時計で答える（#13）。
    #[tokio::test(start_paused = true)]
    async fn the_plan_answers_on_the_wall_clock() {
        let now = at("2026-09-05T12:00:00+09:00");
        let gate = gate(Measure::Gap(Duration::from_secs(900)));

        let plan = gate.plan(Platform::Youtube, Route::Discover, now);
        assert_eq!(plan.next, Next::Now);
        assert_eq!(plan.waiting, 0);

        drop(gate.admit(Demand::weighed(weight(1))).await);

        let plan = gate.plan(Platform::Youtube, Route::Discover, now);
        assert_eq!(plan.next, Next::At(at("2026-09-05T12:15:00+09:00")));
    }

    /// 走っているものが終わる時刻は分からない。分かる顔で答えない。
    #[tokio::test(start_paused = true)]
    async fn a_running_download_has_no_time_to_report() {
        let now = at("2026-09-05T12:00:00+09:00");
        let gate = gate(Measure::Concurrency(weight(1)));
        let held = gate.admit(Demand::weighed(weight(1))).await;

        assert_eq!(
            gate.plan(Platform::X, Route::Acquire, now).next,
            Next::WhenOneFinishes
        );
        drop(held);
    }

    /// 待っている数を数える。UI がこれを読む（#13）。
    #[tokio::test(start_paused = true)]
    async fn the_plan_counts_who_is_waiting() {
        let now = at("2026-09-05T12:00:00+09:00");
        let gate = gate(Measure::Gap(Duration::from_secs(900)));
        drop(gate.admit(Demand::weighed(weight(1))).await);

        let waiting = async {
            tokio::join!(
                passes(&gate, Demand::weighed(weight(1))),
                passes(&gate, Demand::weighed(weight(1))),
            )
        };
        let counted = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            gate.plan(Platform::Youtube, Route::Discover, now).waiting
        };
        let (_, counted) = tokio::join!(waiting, counted);

        assert_eq!(counted, 2);
    }

    /// 片方のプラットフォームの予算が、もう片方を縛らない（#13）。
    #[tokio::test(start_paused = true)]
    async fn one_platform_does_not_block_the_other() {
        let budget = Budget::new(&Schedule::default());
        let demand = Demand::weighed(weight(1));

        let youtube = budget.turn(Platform::Youtube, demand);
        let x = budget.turn(Platform::X, demand);
        let start = Instant::now();

        drop(youtube.admit(Route::Discover).await);
        drop(youtube.admit(Route::Discover).await);
        assert_eq!(Instant::now() - start, Duration::from_secs(900));

        // X 側は一度も使っていないので、待たずに通る。
        drop(x.admit(Route::Discover).await);
        assert_eq!(Instant::now() - start, Duration::from_secs(900));
    }

    /// 同じプラットフォームでも、経路が違えば互いを待たない（#13）。
    #[tokio::test(start_paused = true)]
    async fn one_route_does_not_block_another() {
        let budget = Budget::new(&Schedule::default());
        let turn = budget.turn(Platform::Youtube, Demand::weighed(weight(1)));
        let start = Instant::now();

        drop(turn.admit(Route::Discover).await);
        drop(turn.admit(Route::Describe).await);
        drop(turn.admit(Route::Probe).await);
        drop(turn.admit(Route::Acquire).await);

        assert_eq!(Instant::now() - start, Duration::ZERO);
    }

    /// 予定はプラットフォーム × 経路の全てを答える。
    #[test]
    fn the_plan_covers_every_route_of_every_platform() {
        let budget = Budget::new(&Schedule::default());
        let plans = budget.plan(at("2026-09-05T12:00:00+09:00"));

        assert_eq!(plans.len(), Platform::ALL.len() * Route::ALL.len());
        for platform in Platform::ALL {
            for route in Route::ALL {
                assert!(
                    plans
                        .iter()
                        .any(|p| p.platform == platform && p.route == route),
                    "{platform:?} の {route:?} が無い"
                );
            }
        }
    }
}

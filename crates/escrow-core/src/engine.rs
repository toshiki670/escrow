//! 巡回。#7 の Phase 4。
//!
//! [`crate::pipeline`] が1件を1段進めるのに対し、ここは**どれをいつ進めるか**を
//! 決める。検知・取得・文字起こし・生存確認の4つを回し、`Source` と設定が定めた
//! 間隔と上限を守る。
//!
//! # 時計は引数で入る
//!
//! [`Engine::tick`] も [`Engine::check_liveness`] も `now` を受け取り、自分では
//! 時刻を読まない。眠るのも、どちらをどの間隔で呼ぶかも、外側（`escrow run`）の
//! 仕事。だから #7 の受け入れ条件 —「時計を注入したテストで、期限まで在った →
//! `discarded`、消えた → `kept`、リトライ上限 → `error` が再現する」— が、
//! 実時間を待たずに書ける。
//!
//! # 失敗を一種類にしない
//!
//! 外部ツールの失敗は5通りあり、**台帳の動かし方が変わる**。#7 は「cookie 失効は
//! `Item` を `error` にせず、そのプラットフォームの取得を止めて人に知らせる」と
//! 決めていて、#5 は同じことをツールの仕様変更についても言う。まとめて数えると、
//! こちらの落ち度でない失敗がリトライを食い潰して項目を `error` にする。
//! 振り分けは [`Handling`] が全域関数で持つ。

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::adapter::{AdapterError, Discover, Ports, Probe};
use crate::asset;
use crate::config::Config;
use crate::content::Platform;
use crate::disk::{self, Room};
use crate::hold::HoldDeadline;
use crate::item::{Item, ItemId};
use crate::liveness::Presence;
use crate::pipeline::{Pipeline, PipelineError};
use crate::source::{Source, SourceId};
use crate::state::{Event, State, StateName};
use crate::store::{Applied, Failure, NewItem, Store, StoreError};
use crate::timestamp::Timestamp;
use crate::url;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    #[error("項目 {item} の配信元 {source_id} が台帳に無い")]
    NoSuchSource { item: ItemId, source_id: SourceId },
    #[error("メディアの置き場所を扱えない: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// #2 の設定のうち、エンジンが守るもの。
///
/// [`Config`] をそのまま持たないのは、エンジンが読む項目をここに並べておくと、
/// 設定が増えても巡回の判断に何が効くかが1か所で分かるため。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// これを下回ったら取得を始めない。単位は GiB。
    pub min_free_gib: u32,
    /// これを超えて落ちたら `error`。
    pub max_retries: u32,
}

impl From<&Config> for Limits {
    fn from(config: &Config) -> Self {
        Self {
            min_free_gib: config.storage.min_free_gib,
            max_retries: config.acquire.max_retries,
        }
    }
}

/// 外部ツールが落ちたときに、台帳をどう動かすか。
///
/// [`AdapterError`] の5つを**全部並べる**。受け皿（`_ =>`）を置くと、#7 と #5 が
/// 「`error` にしない」と決めた失敗が、いつのまにかリトライを食い潰して項目を
/// `error` へ落とすようになる。変種が増えたらここがコンパイルエラーになる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handling {
    /// 数えて、次の周でやり直す。`max_retries` を超えたら `error`。
    Retry,
    /// 個別の項目ではなく、そのプラットフォーム全体の問題。取得を止めて人に
    /// 知らせる。項目は動かさず、回数も数えない（#5・#7）。
    Halt,
    /// ツールの仕様が変わった疑い。人に知らせる。項目は動かさず、数えない（#5）。
    Notify,
    /// 配信元から消えていた。やり直しても取れないので、その場で諦める。
    Abandon,
}

const fn handling(error: &AdapterError) -> Handling {
    match error {
        // 起動できない = そのツールが担当するプラットフォームは全部止まる。
        // 項目ごとに数えても意味がない。
        AdapterError::Launch { .. } => Handling::Halt,
        AdapterError::Unauthenticated { .. } => Handling::Halt,
        AdapterError::Parse { .. } => Handling::Notify,
        AdapterError::Unavailable { .. } => Handling::Abandon,
        AdapterError::Transient { .. } => Handling::Retry,
    }
}

/// 人に知らせること。**台帳には残らない。**
///
/// #5 の「cookie の失効は `Item` の状態にしない」がこの型の理由。失効やツールの
/// 仕様変更を `error` として並べると、台帳が「取れなかったもの」で埋まり、本当に
/// 取れなかったものが見えなくなる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// そのプラットフォームの取得を、この周では止めた。
    ///
    /// 次の周でまた試す。止めたことを台帳に持たないので、人が cookie を取り直せば
    /// 何も操作せずに再開する。代わりに、直るまで毎周ここへ出る。
    Halted { platform: Platform, detail: String },
    /// 外部ツールの出力を読めなかった。ツールの仕様が変わった疑い。
    ToolDrifted { detail: String },
    /// 空きが足りないので、この周は取得を始めなかった。
    NoRoom(Room),
}

impl std::fmt::Display for Notice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Halted { platform, detail } => {
                write!(f, "{platform} の取得を止めた: {detail}")
            }
            Self::ToolDrifted { detail } => write!(f, "外部ツールの出力を読めない: {detail}"),
            Self::NoRoom(room) => write!(f, "取得を始めなかった: {room}"),
        }
    }
}

/// 1周の経過。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// 台帳に足した項目。
    pub discovered: usize,
    /// 1段進めた項目。
    pub advanced: usize,
    /// `error` にした項目。
    pub failed: usize,
    /// 生存確認で、配信元から消えていたので手元に残した項目。
    pub kept: usize,
    /// 生存確認で、期限まで在ったので捨てた項目。
    pub discarded: usize,
    /// 人に知らせること。
    pub notices: Vec<Notice>,
}

impl Report {
    /// 別の周ぶんを足し込む。
    ///
    /// `escrow run` は1回の目覚めで [`Engine::tick`] と
    /// [`Engine::check_liveness`] の両方を呼ぶことがある。人へ出すのは
    /// 目覚め1回につき1度でいいので、出す前にここでまとめる。
    pub fn absorb(&mut self, other: Self) {
        self.discovered += other.discovered;
        self.advanced += other.advanced;
        self.failed += other.failed;
        self.kept += other.kept;
        self.discarded += other.discarded;
        self.notices.extend(other.notices);
    }

    /// この周で何かが動いたか。
    pub const fn moved(&self) -> usize {
        self.discovered + self.advanced + self.failed + self.kept + self.discarded
    }

    /// そのプラットフォームを、この周ではもう触らないか。
    ///
    /// 一度 cookie で断られたら、同じ周の残りは全部同じ理由で断られる。
    /// 試すだけ外部ツールを無駄に起動し、通知も同じ数だけ並ぶ。
    pub fn halted(&self, platform: Platform) -> bool {
        self.notices
            .iter()
            .any(|n| matches!(n, Notice::Halted { platform: p, .. } if *p == platform))
    }

    fn note(&mut self, platform: Option<Platform>, error: &AdapterError) -> Handling {
        let handling = handling(error);
        match handling {
            Handling::Halt => {
                if let Some(platform) = platform {
                    self.notices.push(Notice::Halted {
                        platform,
                        detail: error.to_string(),
                    });
                } else {
                    self.notices.push(Notice::ToolDrifted {
                        detail: error.to_string(),
                    });
                }
            }
            Handling::Notify => self.notices.push(Notice::ToolDrifted {
                detail: error.to_string(),
            }),
            Handling::Retry | Handling::Abandon => {}
        }
        handling
    }
}

/// 巡回するもの。
pub struct Engine<'a, P> {
    store: &'a Store,
    ports: &'a P,
    media_dir: &'a Path,
    limits: Limits,
}

impl<'a, P: Ports> Engine<'a, P> {
    pub const fn new(store: &'a Store, ports: &'a P, media_dir: &'a Path, limits: Limits) -> Self {
        Self {
            store,
            ports,
            media_dir,
            limits,
        }
    }

    /// 検知 → 取得 → 文字起こしを1周。
    ///
    /// 生存確認は入らない。#1 が `Source.discover_interval_minutes`（分）と
    /// `check.interval_hours`（時）を別の概念として分けているので、
    /// 回す間隔も別にする（[`Self::check_liveness`]）。
    pub async fn tick(&self, now: Timestamp) -> Result<Report, EngineError> {
        std::fs::create_dir_all(self.media_dir).map_err(|source| EngineError::Io {
            path: self.media_dir.to_owned(),
            source,
        })?;

        let mut report = Report::default();
        self.discover_round(now, &mut report).await?;
        self.acquire_round(now, &mut report).await?;
        self.transcribe_round(now, &mut report).await?;
        Ok(report)
    }

    // ------------------------------------------------------------ 検知ループ

    /// 番の来た配信元を見て、まだ台帳に無いものを足す。
    async fn discover_round(&self, now: Timestamp, report: &mut Report) -> Result<(), EngineError> {
        // #7 の「`Exclude` の適用」。検知の時点で `content_type` は確定するので、
        // 当たったものは**行を作らずに済む**。
        let excludes = self.store.excludes().await?;

        for source in self.store.sources().await? {
            if !source.enabled || !source.due(now) {
                continue;
            }

            let platform = url::platform_of_source(&source.url);
            if platform.is_some_and(|p| report.halted(p)) {
                continue;
            }

            let discoverer = match self.ports.discoverer(&source) {
                Ok(discoverer) => discoverer,
                Err(e) => {
                    report.note(platform, &e);
                    continue;
                }
            };

            let found = match discoverer.discover(&source, source.discover_since()).await {
                Ok(found) => found,
                Err(e) => {
                    report.note(platform, &e);
                    continue;
                }
            };

            for entry in found {
                if excludes
                    .iter()
                    .any(|x| x.covers(source.id, entry.content_type()))
                {
                    continue;
                }
                // `Item.url` は一意キー。前の周と重ねて見ているので、既に在るのが普通。
                if self.store.item_by_url(&entry.url).await?.is_some() {
                    continue;
                }

                self.store
                    .add_item(&NewItem {
                        source_id: source.id,
                        url: entry.url,
                        published_at: entry.published_at,
                        state: State::initial(entry.media),
                        state_since: now,
                        content: entry.content,
                    })
                    .await?;
                report.discovered += 1;
            }

            // 1周通ったときだけ進める。落ちた回に進めると、その回に配信元が
            // 返せなかったものを二度と見に行かなくなる。
            self.store.mark_discovered(source.id, now).await?;
        }

        Ok(())
    }

    // -------------------------------------------------------- 取得の待ち行列

    /// `waiting` と、落ちて `acquiring` のまま残ったものを進める。
    ///
    /// 残ったものを先に片づける。手元に半端な実体を抱えたまま新しい取得を始めると、
    /// 空きが二重に要る。
    async fn acquire_round(&self, now: Timestamp, report: &mut Report) -> Result<(), EngineError> {
        let mut queue = self.store.items_in_state(StateName::Acquiring).await?;
        queue.extend(self.store.items_in_state(StateName::Waiting).await?);

        for item in queue {
            if report.halted(item.content_type().platform()) {
                continue;
            }

            // #2 の門。**始める前に**見る。始めてから足りなくなると、途中まで
            // 書かれた実体と `acquiring` のまま止まった行が残る。
            match self.room()? {
                Room::Enough => {}
                short => {
                    report.notices.push(Notice::NoRoom(short));
                    return Ok(());
                }
            }

            let source = self.source_of(&item).await?;
            let acquirer = self.ports.acquirer(item.content_type());
            let transcriber = self.ports.transcriber();
            let pipeline = Pipeline::new(self.store, self.media_dir, &acquirer, &transcriber);

            match pipeline.acquire(&item, source.hold_policy()).await {
                Ok(_) => report.advanced += 1,
                Err(e) => self.handle(&item, e, now, report).await?,
            }
        }

        Ok(())
    }

    // -------------------------------------------------- 文字起こしの待ち行列

    /// `transcribing` の項目を進める。断片ごとに `video.N` → `transcript.N`（#1）。
    async fn transcribe_round(
        &self,
        now: Timestamp,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        for item in self.store.items_in_state(StateName::Transcribing).await? {
            let source = self.source_of(&item).await?;
            let acquirer = self.ports.acquirer(item.content_type());
            let transcriber = self.ports.transcriber();
            let pipeline = Pipeline::new(self.store, self.media_dir, &acquirer, &transcriber);

            match pipeline.transcribe(&item, source.hold_policy()).await {
                Ok(_) => report.advanced += 1,
                Err(e) => self.handle(&item, e, now, report).await?,
            }
        }

        Ok(())
    }

    // ---------------------------------------------------------- 生存確認バッチ

    /// `holding` の項目が配信元にまだ在るかを確かめる。#2 の `check.interval_hours`。
    ///
    /// 判定は #5 の「生存確認」のとおり、**分類ではなく非対称性**で決まる。
    /// 消えていれば手元のものを残し（`kept`）、期限まで在ったときだけ捨てる
    /// （`discarded`）。分からなければ動かさず、次の回へ回す。
    pub async fn check_liveness(&self, now: Timestamp) -> Result<Report, EngineError> {
        let mut report = Report::default();

        for item in self.store.items_in_state(StateName::Holding).await? {
            let platform = item.content_type().platform();
            if report.halted(platform) {
                continue;
            }

            let source = self.source_of(&item).await?;
            let prober = self.ports.prober(item.content_type());

            let presence = match prober.probe(&item.url).await {
                Ok(presence) => presence,
                Err(e) => {
                    report.note(Some(platform), &e);
                    // 判定そのものは #5 が決める。「消えた」と断定できたときだけ
                    // `Gone` で、それ以外は判定保留。
                    e.presence()
                }
            };

            match (presence, source.hold_days) {
                // 消えた。手元のものを残す。
                (Presence::Gone, _) => {
                    if self.step(&item, &Event::SourceGone, now).await? {
                        report.kept += 1;
                    }
                }
                // 在った。期限を過ぎているときだけ捨てる。証が2つ揃わないと
                // 事象そのものを作れない。
                (Presence::Present, Some(hold_days)) => {
                    let deadline = HoldDeadline::new(item.state_since, hold_days);
                    let witnesses = presence.confirmed().zip(deadline.reached(now));

                    if let Some((presence, deadline)) = witnesses {
                        let event = Event::HeldToDeadline { presence, deadline };
                        if self.step(&item, &event, now).await? {
                            // 台帳を先に更新してからファイルを消す（#7）。
                            asset::remove(self.media_dir, item.id).map_err(|source| {
                                EngineError::Io {
                                    path: asset::item_dir(self.media_dir, item.id),
                                    source,
                                }
                            })?;
                            report.discarded += 1;
                        }
                    }
                }
                // `hold_days` が空の配信元は捨てない（#1）。そもそも `holding` を
                // 通らないので、ここへ来るのは設定が後から変わったとき。
                (Presence::Present, None) => {}
                // 分からないものは捨てない。#5 の「沈黙は確認ではない」。
                (Presence::Unknown, _) => {}
            }
        }

        Ok(report)
    }

    // ------------------------------------------------------------------ 補助

    /// 失敗を振り分ける。項目を `error` にするのは、ここだけ。
    async fn handle(
        &self,
        item: &Item,
        error: PipelineError,
        now: Timestamp,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let error = match error {
            PipelineError::Adapter(e) => e,
            // 読んでから触るまでに誰かが動かした。次の周で読み直す。
            PipelineError::Superseded | PipelineError::NotAtThisStep { .. } => return Ok(()),
            // 台帳と手元の問題は握りつぶさない。
            other => return Err(other.into()),
        };

        let handling = report.note(Some(item.content_type().platform()), &error);
        match handling {
            // 知らせは `note` が積んだ。項目は動かさない。
            Handling::Halt | Handling::Notify => return Ok(()),
            Handling::Abandon | Handling::Retry => {}
        }

        // 段の途中で状態は動いている。`waiting` の項目を渡しても、落ちた時点では
        // もう `acquiring` になっている。数えるのも諦めるのも**いまの行**に対して
        // 行わないと、compare-and-swap が噛み合わず何も起きない。
        let Some(current) = self.store.item(item.id).await? else {
            // 人が消した。次の周で読み直す。
            return Ok(());
        };

        match handling {
            Handling::Abandon => self.give_up(&current, now, report).await?,
            Handling::Retry => {
                match self
                    .store
                    .record_failure(current.id, &current.state)
                    .await?
                {
                    Failure::Superseded => {}
                    Failure::Recorded(attempts) => {
                        // `max_retries` は**やり直しの回数**なので、最初の1回を
                        // 足したぶんだけ落ちて初めて超える。0 なら1回目で `error`。
                        if attempts > self.limits.max_retries {
                            self.give_up(&current, now, report).await?;
                        }
                    }
                }
            }
            Handling::Halt | Handling::Notify => unreachable!("上で返している"),
        }

        Ok(())
    }

    /// もう取れないので `error` へ。
    ///
    /// #1 の図で `acquiring` / `transcribing` から `error` へ行く扉は
    /// `retries_exhausted` ただ1つ。配信元から消えていた場合も同じ扉を通る —
    /// #1 が「理由はログが持ち、ここには載せない」と決めているので、
    /// 理由の数だけ扉を足さない。
    async fn give_up(
        &self,
        item: &Item,
        now: Timestamp,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        if self.step(item, &Event::RetriesExhausted, now).await? {
            report.failed += 1;
        }
        Ok(())
    }

    /// 事象を1つ適用する。書けたら `true`。
    ///
    /// 誰かが先に動かしていたら書かずに `false`。別プロセスの `escrow release` と
    /// ぶつかるのが普通なので、失敗にしない。
    async fn step(&self, item: &Item, event: &Event, now: Timestamp) -> Result<bool, EngineError> {
        Ok(
            match self.store.apply(item.id, &item.state, event, now).await? {
                Applied::Written(_) => true,
                Applied::Superseded => false,
            },
        )
    }

    fn room(&self) -> Result<Room, EngineError> {
        disk::room(self.media_dir, self.limits.min_free_gib).map_err(|source| EngineError::Io {
            path: self.media_dir.to_owned(),
            source,
        })
    }

    async fn source_of(&self, item: &Item) -> Result<Source, EngineError> {
        self.store
            .source(item.source_id)
            .await?
            .ok_or(EngineError::NoSuchSource {
                item: item.id,
                source_id: item.source_id,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{Acquire, Found, Transcribe};
    use crate::asset::{Asset, AssetKind};
    use crate::content::{Content, ContentType, MediaType};
    use crate::source::SourceId;
    use crate::state::MediaPresence;
    use crate::store::NewSource;
    use crate::url::NormalizedUrl;
    use std::num::NonZeroU32;
    use std::sync::Mutex;

    // ------------------------------------------------------------ 外の世界の代役
    //
    // [`Ports`] の4つの trait だけを満たす。#5 の対応表を通らないので、どのツールが
    // 動いたかという概念がそもそも無い — エンジンがツールを知らないことが、
    // ここで確かめられる。

    /// 外部ツールが返すもの。`AdapterError` は `Clone` できないので、
    /// 「何を返させたいか」を持って毎回作る。
    #[derive(Debug, Clone)]
    enum Outcome {
        /// 成功。置くファイル。
        Files(Vec<&'static str>),
        Transient,
        Unauthenticated,
        Unavailable,
        Drifted,
    }

    impl Outcome {
        fn error(&self) -> Option<AdapterError> {
            match self {
                Self::Files(_) => None,
                Self::Transient => Some(AdapterError::Transient {
                    program: "とある道具".to_owned(),
                    detail: "落ちた".to_owned(),
                }),
                Self::Unauthenticated => Some(AdapterError::Unauthenticated {
                    detail: "cookie が失効している".to_owned(),
                }),
                Self::Unavailable => Some(AdapterError::Unavailable {
                    url: "https://example.test/".to_owned(),
                }),
                Self::Drifted => Some(AdapterError::Parse {
                    program: "とある道具".to_owned(),
                    detail: "知らない形".to_owned(),
                }),
            }
        }
    }

    struct World {
        found: Vec<Found>,
        discovery: Outcome,
        acquisition: Outcome,
        presence: Result<Presence, Outcome>,
        acquire_calls: Mutex<usize>,
        probe_calls: Mutex<usize>,
    }

    impl Default for World {
        fn default() -> Self {
            Self {
                found: Vec::new(),
                discovery: Outcome::Files(Vec::new()),
                acquisition: Outcome::Files(vec!["video.1.mp4"]),
                presence: Ok(Presence::Unknown),
                acquire_calls: Mutex::new(0),
                probe_calls: Mutex::new(0),
            }
        }
    }

    /// エンジンへ貸し出す取っ手。#5 の対応表が借りた値を返すのと同じ形。
    struct Handle<'a>(&'a World);

    impl Discover for Handle<'_> {
        async fn discover(
            &self,
            _source: &Source,
            since: Timestamp,
        ) -> Result<Vec<Found>, AdapterError> {
            if let Some(e) = self.0.discovery.error() {
                return Err(e);
            }
            Ok(self
                .0
                .found
                .iter()
                .filter(|f| f.published_at >= since)
                .cloned()
                .collect())
        }
    }

    impl Acquire for Handle<'_> {
        async fn acquire(
            &self,
            _url: &NormalizedUrl,
            _content_type: ContentType,
            into: &Path,
        ) -> Result<Vec<Asset>, AdapterError> {
            *self.0.acquire_calls.lock().unwrap() += 1;

            match &self.0.acquisition {
                Outcome::Files(names) => {
                    std::fs::create_dir_all(into).unwrap();
                    for name in names {
                        std::fs::write(into.join(name), b"x").unwrap();
                    }
                    Ok(asset::scan_dir(into).unwrap())
                }
                other => Err(other.error().expect("成功以外は理由を持つ")),
            }
        }
    }

    impl Transcribe for Handle<'_> {
        async fn transcribe(
            &self,
            _media: &Path,
            into: &Path,
            ordinal: NonZeroU32,
        ) -> Result<Asset, AdapterError> {
            let asset = Asset::new(AssetKind::Transcript, ordinal, "vtt");
            std::fs::write(into.join(asset.file_name()), b"WEBVTT\n").unwrap();
            Ok(asset)
        }
    }

    impl Probe for Handle<'_> {
        async fn probe(&self, _url: &NormalizedUrl) -> Result<Presence, AdapterError> {
            *self.0.probe_calls.lock().unwrap() += 1;
            match &self.0.presence {
                Ok(presence) => Ok(*presence),
                Err(outcome) => Err(outcome.error().expect("失敗は理由を持つ")),
            }
        }
    }

    impl Ports for World {
        fn discoverer(&self, _source: &Source) -> Result<impl Discover, AdapterError> {
            Ok(Handle(self))
        }

        fn acquirer(&self, _content_type: ContentType) -> impl Acquire {
            Handle(self)
        }

        fn prober(&self, _content_type: ContentType) -> impl Probe {
            Handle(self)
        }

        fn transcriber(&self) -> impl Transcribe {
            Handle(self)
        }
    }

    // ---------------------------------------------------------------- 下ごしらえ

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    fn days(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).expect("テストの日数は 1 以上")
    }

    fn limits(max_retries: u32) -> Limits {
        Limits {
            // 門を開けたままにする。閉じた側は専用の試験で見る。
            min_free_gib: 0,
            max_retries,
        }
    }

    async fn source_with(store: &Store, hold_days: Option<NonZeroU32>) -> SourceId {
        enabled_source(store, hold_days, true).await
    }

    async fn enabled_source(
        store: &Store,
        hold_days: Option<NonZeroU32>,
        enabled: bool,
    ) -> SourceId {
        let person = store.add_person("○○").await.unwrap();
        store
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days,
                discover_interval_minutes: NonZeroU32::new(15).unwrap(),
            })
            .await
            .unwrap()
    }

    async fn item_at(store: &Store, source: SourceId, state: State, since: Timestamp) -> ItemId {
        store
            .add_item(&NewItem {
                source_id: source,
                url: url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                    .unwrap()
                    .0,
                published_at: at("2026-03-01T20:00:00+09:00"),
                state,
                state_since: since,
                content: Content::Media {
                    media_type: MediaType::YoutubeVideo,
                    title: "○○の雑談配信".to_owned(),
                },
            })
            .await
            .unwrap()
    }

    fn found(url: &str, published_at: &str, media_type: MediaType) -> Found {
        Found {
            url: url::normalize_item(url).expect(url).0,
            published_at: at(published_at),
            content: Content::Media {
                media_type,
                title: "見つけたもの".to_owned(),
            },
            media: MediaPresence::Present,
        }
    }

    async fn state_of(store: &Store, id: ItemId) -> State {
        store.item(id).await.unwrap().unwrap().state
    }

    // ------------------------------------------------ #7 の受け入れ条件（3件）

    /// 期限まで在った → `discarded`。
    #[tokio::test]
    async fn present_until_the_deadline_is_discarded() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, Some(days(7))).await;
        let id = item_at(
            &store,
            source,
            State::Holding,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;

        let media = tempfile::tempdir().unwrap();
        let dir = asset::item_dir(media.path(), id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("video.1.mp4"), b"x").unwrap();

        let world = World {
            presence: Ok(Presence::Present),
            ..World::default()
        };
        let engine = Engine::new(&store, &world, media.path(), limits(3));

        // 期限の前は動かない。在ることを確かめても、まだ捨てない。
        let report = engine
            .check_liveness(at("2026-03-08T19:59:59+09:00"))
            .await
            .unwrap();
        assert_eq!(report.discarded, 0);
        assert_eq!(state_of(&store, id).await, State::Holding);

        // 期限を過ぎたら捨てる。
        let report = engine
            .check_liveness(at("2026-03-08T20:00:00+09:00"))
            .await
            .unwrap();
        assert_eq!(report.discarded, 1);
        assert_eq!(state_of(&store, id).await, State::Discarded);
        assert!(!dir.exists(), "捨てたら手元の実体も消える");
    }

    /// 消えた → `kept`。
    #[tokio::test]
    async fn gone_from_the_source_is_kept() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, Some(days(7))).await;
        let id = item_at(
            &store,
            source,
            State::Holding,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;

        let media = tempfile::tempdir().unwrap();
        let dir = asset::item_dir(media.path(), id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("video.1.mp4"), b"x").unwrap();

        let world = World {
            presence: Ok(Presence::Gone),
            ..World::default()
        };

        // 期限の前でも、消えていれば手元に残す。
        let report = Engine::new(&store, &world, media.path(), limits(3))
            .check_liveness(at("2026-03-02T00:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(report.kept, 1);
        assert_eq!(state_of(&store, id).await, State::Kept);
        assert!(dir.join("video.1.mp4").is_file(), "残すのだから消さない");
    }

    /// リトライ上限 → `error`。
    #[tokio::test]
    async fn retries_run_out_into_error() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        let id = item_at(
            &store,
            source,
            State::Waiting,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            acquisition: Outcome::Transient,
            ..World::default()
        };
        // やり直し 2 回まで。最初の1回を足して 3 回落ちたら諦める。
        let engine = Engine::new(&store, &world, media.path(), limits(2));

        for round in 1..=2 {
            let report = engine.tick(at("2026-03-01T21:00:00+09:00")).await.unwrap();
            assert_eq!(report.failed, 0, "{round} 回目はまだ諦めない");
            assert_eq!(state_of(&store, id).await, State::Acquiring);
        }

        let report = engine.tick(at("2026-03-01T21:00:00+09:00")).await.unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(state_of(&store, id).await, State::Error);
        assert_eq!(*world.acquire_calls.lock().unwrap(), 3);
    }

    /// `max_retries = 0` は「やり直さない」。1回目で諦める。
    #[tokio::test]
    async fn zero_retries_gives_up_on_the_first_failure() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        let id = item_at(
            &store,
            source,
            State::Waiting,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            acquisition: Outcome::Transient,
            ..World::default()
        };
        Engine::new(&store, &world, media.path(), limits(0))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(state_of(&store, id).await, State::Error);
    }

    // ---------------------------------------------- 失敗を一種類にしないこと

    /// #7 の「cookie 失効の検知」。**`Item` を `error` にしない。**
    ///
    /// 何周繰り返しても状態は動かない。回数を数えていないので、`max_retries` を
    /// いくら超えても `error` にならない。
    #[tokio::test]
    async fn a_dead_cookie_never_marks_items_as_errors() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        let id = item_at(
            &store,
            source,
            State::Waiting,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            acquisition: Outcome::Unauthenticated,
            ..World::default()
        };
        let engine = Engine::new(&store, &world, media.path(), limits(0));

        for _ in 0..5 {
            let report = engine.tick(at("2026-03-01T21:00:00+09:00")).await.unwrap();
            assert_eq!(report.failed, 0);
            assert!(
                report.notices.iter().any(|n| matches!(
                    n,
                    Notice::Halted {
                        platform: Platform::Youtube,
                        ..
                    }
                )),
                "止めたことは毎周知らせる"
            );
        }

        assert_ne!(state_of(&store, id).await, State::Error);
    }

    /// 止めたプラットフォームは、その周のうちは二度と叩かない。
    #[tokio::test]
    async fn a_halted_platform_is_not_tried_again_in_the_same_round() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        for n in 1..=3 {
            store
                .add_item(&NewItem {
                    source_id: source,
                    url: url::normalize_item(&format!(
                        "https://www.youtube.com/watch?v=aaaaaaaaaa{n}"
                    ))
                    .unwrap()
                    .0,
                    published_at: at("2026-03-01T20:00:00+09:00"),
                    state: State::Waiting,
                    state_since: at("2026-03-01T20:00:00+09:00"),
                    content: Content::Media {
                        media_type: MediaType::YoutubeVideo,
                        title: format!("{n} 本目"),
                    },
                })
                .await
                .unwrap();
        }
        let media = tempfile::tempdir().unwrap();

        let world = World {
            acquisition: Outcome::Unauthenticated,
            ..World::default()
        };
        let report = Engine::new(&store, &world, media.path(), limits(3))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(*world.acquire_calls.lock().unwrap(), 1, "1件で止める");
        assert_eq!(report.notices.len(), 1, "知らせも1つ");
    }

    /// 検知で止めたら、同じ周の取得も止まる。段をまたいで効くこと。
    #[tokio::test]
    async fn halting_during_discovery_also_stops_acquisition() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        let id = item_at(
            &store,
            source,
            State::Waiting,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            discovery: Outcome::Unauthenticated,
            // 取得そのものは通るはずの設定。それでも触らない。
            acquisition: Outcome::Files(vec!["video.1.mp4"]),
            ..World::default()
        };
        let report = Engine::new(&store, &world, media.path(), limits(3))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(*world.acquire_calls.lock().unwrap(), 0);
        assert_eq!(report.notices.len(), 1, "知らせも段ごとに増えない");
        assert_eq!(state_of(&store, id).await, State::Waiting);
    }

    /// ツールの仕様が変わった疑いも `error` にしない（#5）。
    #[tokio::test]
    async fn a_tool_that_drifted_is_reported_not_counted() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        let id = item_at(
            &store,
            source,
            State::Waiting,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            acquisition: Outcome::Drifted,
            ..World::default()
        };
        let report = Engine::new(&store, &world, media.path(), limits(0))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(report.failed, 0);
        assert!(
            report
                .notices
                .iter()
                .any(|n| matches!(n, Notice::ToolDrifted { .. }))
        );
        assert_ne!(state_of(&store, id).await, State::Error);
    }

    /// 配信元から消えていたら、やり直さずその場で諦める。
    #[tokio::test]
    async fn something_gone_before_we_could_fetch_it_is_not_retried() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        let id = item_at(
            &store,
            source,
            State::Waiting,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            acquisition: Outcome::Unavailable,
            ..World::default()
        };
        // やり直しを 10 回許していても、1周で諦める。
        let report = Engine::new(&store, &world, media.path(), limits(10))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(report.failed, 1);
        assert_eq!(state_of(&store, id).await, State::Error);
    }

    /// 生存確認で判定がつかないものは動かさない。#5 の「沈黙は確認ではない」。
    #[tokio::test]
    async fn silence_never_discards() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, Some(days(1))).await;
        let id = item_at(
            &store,
            source,
            State::Holding,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;
        let media = tempfile::tempdir().unwrap();

        // 期限をとうに過ぎていても、確かめられなければ捨てない。
        let long_after = at("2026-06-01T00:00:00+09:00");

        for presence in [Ok(Presence::Unknown), Err(Outcome::Unauthenticated)] {
            let world = World {
                presence,
                ..World::default()
            };
            let report = Engine::new(&store, &world, media.path(), limits(3))
                .check_liveness(long_after)
                .await
                .unwrap();

            assert_eq!(report.discarded, 0);
            assert_eq!(state_of(&store, id).await, State::Holding);
        }
    }

    // -------------------------------------------------------------- 空きの門

    /// 空きが足りなければ取得を始めない。**失敗としては数えない。**
    #[tokio::test]
    async fn the_gate_holds_items_without_failing_them() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        let id = item_at(
            &store,
            source,
            State::Waiting,
            at("2026-03-01T20:00:00+09:00"),
        )
        .await;
        let media = tempfile::tempdir().unwrap();

        let world = World::default();
        let closed = Limits {
            // どの区画にもこれだけの空きは無い（1024 EiB）。
            min_free_gib: u32::MAX,
            max_retries: 0,
        };
        let report = Engine::new(&store, &world, media.path(), closed)
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(*world.acquire_calls.lock().unwrap(), 0);
        assert_eq!(report.failed, 0);
        assert_eq!(
            state_of(&store, id).await,
            State::Waiting,
            "門が開けば流れる"
        );
        assert!(
            report
                .notices
                .iter()
                .any(|n| matches!(n, Notice::NoRoom(Room::Short { .. })))
        );
    }

    // ---------------------------------------------------------- 検知ループ

    /// 見つけたものを台帳へ足し、そのまま取得まで運ぶ。
    #[tokio::test]
    async fn discovery_feeds_the_queue() {
        let store = Store::open_in_memory().await.unwrap();
        source_with(&store, None).await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            found: vec![found(
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "2026-03-01T20:00:00+09:00",
                MediaType::YoutubeVideo,
            )],
            ..World::default()
        };
        let report = Engine::new(&store, &world, media.path(), limits(3))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(report.discovered, 1);
        // 同じ周のうちに取得と文字起こしまで進む。
        assert_eq!(report.advanced, 2);
        assert_eq!(
            store.items_in_state(StateName::Kept).await.unwrap().len(),
            1
        );
    }

    /// 番が来るまで叩かない。前の周と重ねて見るので、`since` は間隔ぶん戻る。
    #[tokio::test]
    async fn a_source_is_only_visited_when_its_turn_comes() {
        let store = Store::open_in_memory().await.unwrap();
        let id = source_with(&store, None).await;
        let media = tempfile::tempdir().unwrap();
        let world = World::default();
        let engine = Engine::new(&store, &world, media.path(), limits(3));

        let first = at("2026-03-01T21:00:00+09:00");
        engine.tick(first).await.unwrap();

        let source = store.source(id).await.unwrap().unwrap();
        assert_eq!(source.last_discovered_at, Some(first));
        assert!(!source.due(at("2026-03-01T21:14:59+09:00")));
        assert!(source.due(at("2026-03-01T21:15:00+09:00")));
        // 15 分の間隔ぶん重ねて見る。
        assert_eq!(source.discover_since(), at("2026-03-01T20:45:00+09:00"));
    }

    /// 落ちた周は進めない。進めると、その回に配信元が返せなかったものを
    /// 二度と見に行かなくなる。
    #[tokio::test]
    async fn a_failed_round_does_not_advance_the_mark() {
        let store = Store::open_in_memory().await.unwrap();
        let id = source_with(&store, None).await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            discovery: Outcome::Transient,
            ..World::default()
        };
        Engine::new(&store, &world, media.path(), limits(3))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        let source = store.source(id).await.unwrap().unwrap();
        assert_eq!(source.last_discovered_at, None);
        assert_eq!(source.discover_since(), source.created_at);
    }

    /// 止めた配信元は見に行かない。
    #[tokio::test]
    async fn a_disabled_source_is_skipped() {
        let store = Store::open_in_memory().await.unwrap();
        let id = enabled_source(&store, None, false).await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            found: vec![found(
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "2026-03-01T20:00:00+09:00",
                MediaType::YoutubeVideo,
            )],
            ..World::default()
        };
        let report = Engine::new(&store, &world, media.path(), limits(3))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(report.discovered, 0);
        assert_eq!(
            store.source(id).await.unwrap().unwrap().last_discovered_at,
            None
        );
    }

    /// #7 の「`Exclude` の適用」。当たったものは**行を作らない**。
    #[tokio::test]
    async fn an_excluded_type_never_becomes_a_row() {
        let store = Store::open_in_memory().await.unwrap();
        let source = source_with(&store, None).await;
        store
            .add_exclude(Some(source), ContentType::YoutubeShorts, true)
            .await
            .unwrap();
        let media = tempfile::tempdir().unwrap();

        let world = World {
            found: vec![
                found(
                    "https://www.youtube.com/shorts/dQw4w9WgXcQ",
                    "2026-03-01T20:00:00+09:00",
                    MediaType::YoutubeShorts,
                ),
                found(
                    "https://www.youtube.com/watch?v=aaaaaaaaaaa",
                    "2026-03-01T20:00:00+09:00",
                    MediaType::YoutubeVideo,
                ),
            ],
            ..World::default()
        };
        let report = Engine::new(&store, &world, media.path(), limits(3))
            .tick(at("2026-03-01T21:00:00+09:00"))
            .await
            .unwrap();

        assert_eq!(report.discovered, 1, "ショートは行にならない");
    }

    /// 同じものを二度足さない。前の周と重ねて見ているので、毎周これが効く。
    #[tokio::test]
    async fn seeing_the_same_thing_twice_adds_one_row() {
        let store = Store::open_in_memory().await.unwrap();
        source_with(&store, None).await;
        let media = tempfile::tempdir().unwrap();

        let world = World {
            found: vec![found(
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "2026-03-01T20:00:00+09:00",
                MediaType::YoutubeVideo,
            )],
            ..World::default()
        };
        let engine = Engine::new(&store, &world, media.path(), limits(3));

        assert_eq!(
            engine
                .tick(at("2026-03-01T21:00:00+09:00"))
                .await
                .unwrap()
                .discovered,
            1
        );
        assert_eq!(
            engine
                .tick(at("2026-03-01T22:00:00+09:00"))
                .await
                .unwrap()
                .discovered,
            0
        );
    }

    // ------------------------------------------------------------ 振り分け

    /// [`AdapterError`] の5つが、それぞれ違う扱いを受けること。
    /// 変種を足したらここが落ちる。
    #[test]
    fn every_kind_of_failure_has_its_own_handling() {
        let cases = [
            (Outcome::Transient, Handling::Retry),
            (Outcome::Unauthenticated, Handling::Halt),
            (Outcome::Drifted, Handling::Notify),
            (Outcome::Unavailable, Handling::Abandon),
        ];

        for (outcome, expected) in cases {
            let error = outcome.error().expect("失敗は理由を持つ");
            assert_eq!(handling(&error), expected, "{error}");
        }

        // 起動できないものは、その道具が担当するぶんが全部止まる。
        let launch = AdapterError::Launch {
            program: "とある道具".to_owned(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        assert_eq!(handling(&launch), Handling::Halt);
    }
}

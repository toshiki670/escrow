//! 状態と、それを動かす事象。
//!
//! 台帳を読むのは async なので、読み出しは [`Task`] として出し、結果を [`Message`] で
//! 受け取る。失敗は文字列にして運ぶ — iced の [`Message`] は複製できることを求めるが、
//! 台帳と設定の失敗はどちらも複製できない。**画面が出すのは理由の文だけ**なので、
//! 型を保ったまま運ぶ意味がここには無い。

use std::path::PathBuf;
use std::sync::Arc;

use escrow_config::{Config, Dirs, Paths};
use escrow_domain::source::{Person, PersonId};
use escrow_handover::Handover;
use escrow_ledger::Ledger;
use iced::Task;

use crate::listing::{self, Listed};

/// サイドバーで選べるもの（#6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Dashboard,
    Person(PersonId),
    Settings,
}

/// 画面の全体。台帳を開くまでは、まだ何も並べられない。
pub enum App {
    Opening,
    /// 台帳を開けなかった。直す先は設定の場所か DB で、画面はその理由を出すだけ。
    Unavailable(String),
    Ready(Ready),
}

/// 台帳を開いたあと。
pub struct Ready {
    ledger: Arc<Ledger>,
    media_dir: PathBuf,
    persons: Vec<Person>,
    selection: Selection,
    listing: Listing,
}

/// `Person` を選んだときのメイン。
pub enum Listing {
    /// 読んでいる最中。`Person` 以外を選んでいる間もここに居る。
    Loading,
    Loaded(Vec<Listed>),
    Failed(String),
}

/// 開いた台帳と、その中身を読むのに要るもの。
#[derive(Debug, Clone)]
pub struct Opened {
    ledger: Arc<Ledger>,
    media_dir: PathBuf,
    persons: Vec<Person>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// 台帳を開き、持ち主を読み終えた。
    Opened(Result<Opened, String>),
    /// サイドバーで選んだ。
    Selected(Selection),
    /// 項目を読み終えた。**どの持ち主のぶんか**を連れて戻る。
    Listed(PersonId, Result<Vec<Listed>, String>),
}

/// 台帳を開くのは async なので、最初の状態と一緒に読み出しを出す。
pub fn boot() -> (App, Task<Message>) {
    (App::Opening, Task::perform(open(), Message::Opened))
}

pub fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::Opened(Ok(opened)) => {
            *app = App::Ready(Ready {
                ledger: opened.ledger,
                media_dir: opened.media_dir,
                persons: opened.persons,
                selection: Selection::Dashboard,
                listing: Listing::Loading,
            });
            Task::none()
        }
        Message::Opened(Err(why)) => {
            *app = App::Unavailable(why);
            Task::none()
        }
        Message::Selected(selection) => {
            let App::Ready(ready) = app else {
                return Task::none();
            };
            ready.selection = selection;

            match selection {
                Selection::Person(person) => {
                    ready.listing = Listing::Loading;
                    let ledger = Arc::clone(&ready.ledger);
                    let media_dir = ready.media_dir.clone();

                    Task::perform(list(ledger, media_dir, person), move |listed| {
                        Message::Listed(person, listed)
                    })
                }
                Selection::Dashboard | Selection::Settings => Task::none(),
            }
        }
        Message::Listed(person, listed) => {
            let App::Ready(ready) = app else {
                return Task::none();
            };
            // 読んでいる間に選び直されていたら、届いたぶんは捨てる。
            if ready.selection != Selection::Person(person) {
                return Task::none();
            }

            ready.listing = match listed {
                Ok(listed) => Listing::Loaded(listed),
                Err(why) => Listing::Failed(why),
            };
            Task::none()
        }
    }
}

impl Ready {
    pub fn persons(&self) -> &[Person] {
        &self.persons
    }

    pub const fn selection(&self) -> Selection {
        self.selection
    }

    pub const fn listing(&self) -> &Listing {
        &self.listing
    }

    /// 選んでいる持ち主。`Person` 以外を選んでいるなら空。
    pub fn selected_person(&self) -> Option<&Person> {
        match self.selection {
            Selection::Person(id) => self.persons.iter().find(|person| person.id == id),
            Selection::Dashboard | Selection::Settings => None,
        }
    }
}

/// 設定の言う場所で台帳を開き、持ち主を並べる。
///
/// 開けなければサイドバーも出せないので、ここが通るまで画面は空のまま。
async fn open() -> Result<Opened, String> {
    let dirs = Dirs::discover().map_err(why)?;
    let config = Config::load(&dirs.config_file()).map_err(why)?;
    let paths = Paths::resolve(&config, &dirs);

    let ledger = Ledger::open(&paths.db).await.map_err(why)?;
    let persons = ledger.persons().await.map_err(why)?;

    Ok(Opened {
        ledger: Arc::new(ledger),
        media_dir: paths.media_dir,
        persons,
    })
}

/// 選んだ持ち主の項目を、一覧に出す形まで写す。
///
/// 1件ごとに手元の実体を数えるので、行の数だけディレクトリを見に行く。#4 の
/// `list` と同じ経路で、**画面に出る値と外部が受け取る値をずらさない**ための代償。
async fn list(
    ledger: Arc<Ledger>,
    media_dir: PathBuf,
    person: PersonId,
) -> Result<Vec<Listed>, String> {
    let projected = ledger.items_of_person(person).await.map_err(why)?;
    let handover = Handover::new(&ledger, &media_dir);

    let mut listed = projected
        .iter()
        .map(|projected| {
            handover
                .handed(&projected.item)
                .map(|handed| Listed::new(projected.item.published_at, &handed))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(why)?;

    listing::newest_first(&mut listed);
    Ok(listed)
}

fn why(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use escrow_domain::content::{Content, MediaType};
    use escrow_domain::item::Discovered;
    use escrow_domain::source::{Monitoring, Person, SourceId};
    use escrow_domain::state::{Event, Hold, MediaPresence, TranscriptNeed};
    use escrow_domain::timestamp::Timestamp;
    use escrow_domain::url;
    use escrow_ledger::{NewSource, Seq};
    use iced_test::simulator;

    use super::*;
    use crate::view::view;

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).expect(text)
    }

    async fn a_source_for(ledger: &Ledger, person: PersonId, raw: &str) -> SourceId {
        ledger
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source(raw).expect(raw),
                enabled: true,
                created_at: at("2026-01-01T00:00:00+09:00"),
                hold_days: NonZeroU32::new(7),
                priority: NonZeroU32::MIN,
                monitoring: Monitoring::Continuous,
            })
            .await
            .unwrap()
    }

    /// CLI の `person add` → `source add` → `item add` → `fetch` が作る形を、台帳へ直接置く。
    ///
    /// ○○ は配信1本（`holding`）と投稿1件（`kept`）、□□ は項目を持たない。
    ///
    /// **新しいほうを後から見つける。** 投影は id の順で返るので、こうしないと
    /// 「並べ替えを忘れた」が「たまたま合っている」に化ける。
    async fn seeded() -> (Ledger, Vec<Person>) {
        let ledger = Ledger::open_in_memory().await.unwrap();

        let owner = ledger.add_person("○○").await.unwrap();
        let youtube = a_source_for(
            &ledger,
            owner,
            "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
        )
        .await;
        let x = a_source_for(&ledger, owner, "https://x.com/i/user/12").await;

        // 本文だけの投稿は、取るものが無いのでそのまま kept から始まる（#1）。
        ledger
            .discover(
                &Discovered {
                    source_id: x,
                    url: url::normalize_item("https://x.com/jack/status/20")
                        .unwrap()
                        .0,
                    published_at: at("2026-03-01T12:00:00+09:00"),
                    scheduled_start_at: None,
                    content: Content::Post {
                        body: "明日の配信は21時から。".to_owned(),
                        in_reply_to: None,
                        quoted: None,
                    },
                    media: MediaPresence::Absent,
                },
                at("2026-03-01T12:01:00+09:00"),
            )
            .await
            .unwrap();

        let live = ledger
            .discover(
                &Discovered {
                    source_id: youtube,
                    url: url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                        .unwrap()
                        .0,
                    published_at: at("2026-03-01T20:00:00+09:00"),
                    scheduled_start_at: None,
                    content: Content::Media {
                        media_type: MediaType::YoutubeLive,
                        title: "○○の雑談配信".to_owned(),
                    },
                    media: MediaPresence::Present,
                },
                at("2026-03-01T20:05:00+09:00"),
            )
            .await
            .unwrap();
        let seq = ledger
            .append(
                live,
                Seq::FIRST,
                &Event::AcquisitionStarted,
                at("2026-03-01T20:10:00+09:00"),
            )
            .await
            .unwrap();
        ledger
            .append(
                live,
                seq,
                &Event::Acquired {
                    transcript: TranscriptNeed::NotNeeded,
                    hold: Hold::Until(at("2026-03-09T00:30:00+09:00")),
                },
                at("2026-03-02T00:30:00+09:00"),
            )
            .await
            .unwrap();

        ledger.add_person("□□").await.unwrap();

        let persons = ledger.persons().await.unwrap();
        (ledger, persons)
    }

    /// 台帳を開き終えた画面。実体の置き場所は空で、手元に何も無い状態を表す。
    fn opened(ledger: Ledger, media_dir: &std::path::Path, persons: Vec<Person>) -> App {
        let mut app = App::Opening;
        let _ = update(
            &mut app,
            Message::Opened(Ok(Opened {
                ledger: Arc::new(ledger),
                media_dir: media_dir.to_owned(),
                persons,
            })),
        );
        app
    }

    /// サイドバーの1つを押し、そこから走る読み出しまで流す。
    ///
    /// 出荷時に [`Task`] を走らせるのは iced なので、ここではテストが同じことをする。
    async fn press(app: &mut App, name: &str) {
        let clicked: Vec<Message> = {
            let mut ui = simulator(view(app));
            ui.click(name)
                .unwrap_or_else(|e| panic!("サイドバーに {name} が無い: {e:?}"));
            ui.into_messages().collect()
        };

        for message in clicked {
            let task = update(app, message);
            for message in run(task).await {
                let _ = update(app, message);
            }
        }
    }

    /// [`Task`] を最後まで回して、出てきた事象を集める。
    async fn run(task: Task<Message>) -> Vec<Message> {
        use iced::futures::StreamExt as _;
        use iced_test::runtime::{Action, task};

        let Some(stream) = task::into_stream(task) else {
            return Vec::new();
        };

        stream
            .filter_map(|action| async move {
                match action {
                    Action::Output(message) => Some(message),
                    _ => None,
                }
            })
            .collect()
            .await
    }

    fn shows(app: &App, text: &str) -> bool {
        simulator(view(app)).find(text).is_ok()
    }

    /// 描いた画面での、その文字の上端。
    ///
    /// 並び順は `Listed` の順ではなく**描いた結果**で見る。一覧まで届いていない
    /// 並べ替えは、持っているだけで誰の役にも立たない。
    fn top_of(app: &App, text: &str) -> f32 {
        simulator(view(app))
            .find(text)
            .unwrap_or_else(|e| panic!("画面に {text} が無い: {e:?}"))
            .bounds()
            .y
    }

    /// #30 の受け入れ — 入れた項目が `Person` を選んだときの一覧に出る。
    /// 状態と種別は #1 の表の値そのまま。
    #[tokio::test]
    async fn the_items_of_the_selected_person_appear_in_the_list() {
        let (ledger, persons) = seeded().await;
        let media = tempfile::tempdir().unwrap();
        let mut app = opened(ledger, media.path(), persons);

        // #6 の骨格。サイドバーはダッシュボードと持ち主と設定だけ。
        assert!(shows(&app, "ダッシュボード"));
        assert!(shows(&app, "設定"));
        assert!(shows(&app, "○○"));
        assert!(shows(&app, "□□"));

        press(&mut app, "○○").await;

        // 見出しは Media なら title、Post なら body の先頭（#6）。
        // 新しいものが上（#6 のモック）。配信は 20:00+09:00、投稿は 12:00+09:00。
        assert!(
            top_of(&app, "○○の雑談配信") < top_of(&app, "明日の配信は21時から。"),
            "新しい項目が上に来る"
        );
        // 状態と種別は #1 の表の値。
        assert!(shows(&app, "holding"));
        assert!(shows(&app, "kept"));
        assert!(shows(&app, "youtube_live"));
        assert!(shows(&app, "x_post"));
        // 日付は日付だけ。
        assert!(shows(&app, "2026-03-01"));
    }

    /// #30 の受け入れ — 項目が0件の `Person` でも画面が壊れない。
    #[tokio::test]
    async fn a_person_without_items_still_draws() {
        let (ledger, persons) = seeded().await;
        let media = tempfile::tempdir().unwrap();
        let mut app = opened(ledger, media.path(), persons);

        press(&mut app, "□□").await;

        assert!(shows(&app, "□□"));
        assert!(shows(&app, "項目はまだ無い"));
    }
}

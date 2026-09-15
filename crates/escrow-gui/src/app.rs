//! 状態と、それを動かす事象。
//!
//! 台帳を読むのは async なので、読み出しは [`Task`] として出し、結果を [`Message`] で
//! 受け取る。失敗は文字列にして運ぶ — iced の [`Message`] は複製できることを求めるが、
//! 台帳と設定の失敗はどちらも複製できない。**画面が出すのは理由の文だけ**なので、
//! 型を保ったまま運ぶ意味がここには無い。
//!
//! 台帳を開く手順と読む関数は `escrow-app` が持つ（#82）。ここに在るのは、画面の状態と
//! 事象の往復だけ。

use std::sync::Arc;

use escrow_app::{Listed, Person, PersonId};
use iced::Task;

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
    app: Arc<escrow_app::App>,
    persons: Vec<Person>,
    selection: Selection,
    listing: Listing,
}

/// `Person` を選んだときのメイン。
///
/// ダッシュボードと設定を選んでも、直前に読んだ中身がそのまま残る。そちらは
/// 一覧を出さないので画面には現れない。
pub enum Listing {
    /// 読んでいる最中。台帳を開いた直後もここから始まる。
    Loading,
    Loaded(Vec<Listed>),
    Failed(String),
}

/// 開いた台帳と、その中身を読むのに要るもの。
#[derive(Debug, Clone)]
pub struct Opened {
    app: Arc<escrow_app::App>,
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
                app: opened.app,
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
                    let app = Arc::clone(&ready.app);

                    Task::perform(
                        async move { app.items_of(person).await.map_err(why) },
                        move |listed| Message::Listed(person, listed),
                    )
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
    let app = escrow_app::App::open().await.map_err(why)?;
    let persons = app.persons().await.map_err(why)?;

    Ok(Opened {
        app: Arc::new(app),
        persons,
    })
}

/// 失敗の理由を、原因まで繋いで1つの文にする。
///
/// `escrow-app` の失敗は「設定を読めない」のように段階を言い、直す先（設定ファイルか
/// DB か）を言うのは原因の側。そこまで出して、画面が直す先を示す。
fn why(error: impl std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        text.push_str(": ");
        text.push_str(&source.to_string());
        cause = source.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use iced_test::simulator;

    use super::*;
    use crate::view::view;

    /// 台帳を開き終えた画面。実体の置き場所は空で、手元に何も無い状態を表す。
    ///
    /// 台帳へ置く形は [`escrow_app::App::seeded`] が決める（#82）。
    async fn opened(media_dir: &Path) -> App {
        let assembled = escrow_app::App::seeded(media_dir).await;
        let persons = assembled.persons().await.unwrap();

        let mut app = App::Opening;
        let _ = update(
            &mut app,
            Message::Opened(Ok(Opened {
                app: Arc::new(assembled),
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
        let media = tempfile::tempdir().unwrap();
        let mut app = opened(media.path()).await;

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
        let media = tempfile::tempdir().unwrap();
        let mut app = opened(media.path()).await;

        press(&mut app, "□□").await;

        assert!(shows(&app, "□□"));
        assert!(shows(&app, "項目はまだ無い"));
    }

    /// 失敗は原因まで繋いで出す。直す先を言うのは原因の側。
    #[test]
    fn a_failure_reads_down_to_its_cause() {
        /// 「段階: 原因」の2段。`escrow-app` の `Config` / `Open` と同じ形。
        #[derive(Debug)]
        struct Staged(&'static str, Option<Box<Staged>>);

        impl std::fmt::Display for Staged {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for Staged {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.1.as_deref().map(|cause| cause as _)
            }
        }

        let failure = Staged(
            "設定を読めない",
            Some(Box::new(Staged("設定ファイルを TOML として読めない", None))),
        );
        assert_eq!(
            why(failure),
            "設定を読めない: 設定ファイルを TOML として読めない"
        );
    }
}

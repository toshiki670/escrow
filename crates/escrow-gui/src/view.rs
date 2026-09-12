//! #6 の骨格 — サイドバー（ダッシュボード ＋ `Person` ＋ 設定）とメイン。
//!
//! ダッシュボードと設定は項目だけ置く。中身は別のスライス（#30）。

use escrow_domain::source::Person;
use iced::widget::{button, column, container, row, rule, scrollable, space, table, text};
use iced::{Element, Fill};

use crate::app::{App, Listing, Message, Ready, Selection};
use crate::listing::Listed;

/// サイドバーの幅。#6 の骨格のとおり、メインより狭い固定幅。
const SIDEBAR_WIDTH: f32 = 200.0;

pub fn view(app: &App) -> Element<'_, Message> {
    match app {
        App::Opening => centered("台帳を開いています"),
        App::Unavailable(why) => centered(format!("台帳を開けません — {why}")),
        App::Ready(ready) => row![sidebar(ready), main(ready)].into(),
    }
}

fn sidebar(ready: &Ready) -> Element<'_, Message> {
    let persons = ready
        .persons()
        .iter()
        .map(|person| entry(&person.name, Selection::Person(person.id), ready));

    container(
        column![
            entry("ダッシュボード", Selection::Dashboard, ready),
            space().height(12),
            column(persons).spacing(2),
            space().height(Fill),
            entry("設定", Selection::Settings, ready),
        ]
        .spacing(2),
    )
    .width(SIDEBAR_WIDTH)
    .height(Fill)
    .padding(12)
    .style(container::rounded_box)
    .into()
}

/// サイドバーの1項目。選んでいるものだけ塗る。
fn entry<'a>(name: &'a str, selection: Selection, ready: &Ready) -> Element<'a, Message> {
    let style = if selection == ready.selection() {
        button::secondary
    } else {
        button::text
    };

    button(text(name))
        .width(Fill)
        .style(style)
        .on_press(Message::Selected(selection))
        .into()
}

/// #6 の「メインコンテンツ」。サイドバーで選んだものを出す。
fn main(ready: &Ready) -> Element<'_, Message> {
    let body = match ready.selection() {
        Selection::Dashboard => centered("ダッシュボードは別のスライスで入る"),
        Selection::Settings => centered("設定は別のスライスで入る"),
        Selection::Person(_) => match ready.selected_person() {
            Some(person) => items_of(person, ready.listing()),
            // 持ち主を読んだあとに消えた。次に開けば居なくなっている。
            None => centered("この持ち主は台帳に居ない"),
        },
    };

    container(body).width(Fill).height(Fill).padding(20).into()
}

fn items_of<'a>(person: &'a Person, listing: &'a Listing) -> Element<'a, Message> {
    let body: Element<'a, Message> = match listing {
        Listing::Loading => text("読んでいます").into(),
        Listing::Failed(why) => text(format!("項目を読めません — {why}")).into(),
        Listing::Loaded(listed) if listed.is_empty() => text("項目はまだ無い").into(),
        Listing::Loaded(listed) => scrollable(items(listed)).height(Fill).into(),
    };

    column![
        text(&person.name).size(24),
        rule::horizontal(1),
        space().height(8),
        body,
    ]
    .spacing(4)
    .into()
}

/// 項目の一覧 — 日付・見出し・状態・種別（#6）。
fn items(listed: &[Listed]) -> Element<'_, Message> {
    table(
        [
            table::column(text("日付"), |item: &Listed| text(item.published_on())).width(110.0),
            table::column(text("項目"), |item: &Listed| text(item.headline())).width(Fill),
            table::column(text("状態"), |item: &Listed| text(item.state())).width(110.0),
            table::column(text("種別"), |item: &Listed| text(item.content_type())).width(140.0),
        ],
        listed,
    )
    .width(Fill)
    .padding_y(6)
    // 行の区切りだけ引く。#6 のモックに枠は無いが、1行が4列に分かれるので
    // 横の区切りが無いと、どこまでが1件かが読めない。
    .separator_x(0)
    .separator_y(1)
    .into()
}

fn centered<'a>(message: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    container(text(message))
        .width(Fill)
        .height(Fill)
        .center(Fill)
        .into()
}

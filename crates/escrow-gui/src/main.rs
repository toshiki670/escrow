//! escrow の GUI。#6 の骨格を出し、`Person` ごとの項目一覧を見せる（#30）。
//!
//! 読むのは投影だけ。事象を書く道と巡回するエンジンは別のスライスが入れるので、
//! 項目を入れるのは CLI の `person add` → `source add` → `item add` → `fetch`。

mod app;
mod listing;
mod view;

fn main() -> iced::Result {
    iced::application(app::boot, app::update, view::view)
        .title("escrow")
        .window_size((960.0, 640.0))
        .run()
}

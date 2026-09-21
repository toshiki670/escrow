//! Rust 以外から `escrow-app` を呼ぶ入口（#79）。
//!
//! UniFFI がここの公開 API から Swift の関数を生成する。`escrow-app` の型のうち、UniFFI が
//! 運べる形のものはそのまま通し（`Person` / `Headline` は写し、`App` は handle）、運べない
//! ものだけをここで詰め替える（`Listed` は accessor しか無いので record へ、`AppError` は
//! 文へ）。
//!
//! `App` のメソッドは関数になる。UniFFI は写した Object へのメソッドの export をまだ持たない
//! （uniffi-rs の `examples/remote-types` が TODO に挙げている）。

use std::sync::Arc;

use escrow_app::{Headline, Person, PersonId};

/// [`escrow_app::App`]。Swift 側の名前は `App` にしない — SwiftUI の `App` protocol と衝突する。
type Escrow = escrow_app::App;

uniffi::setup_scaffolding!();

// 識別子は i64 で運ぶ。`escrow-app` の引数が `i64` で受けるのと同じ形（#82）。
uniffi::custom_type!(PersonId, i64, {
    remote,
    lower: |id| id.into(),
    try_lift: |raw| Ok(PersonId::new(raw)),
});

#[uniffi::remote(Record)]
pub struct Person {
    pub id: PersonId,
    pub name: String,
}

#[uniffi::remote(Enum)]
pub enum Headline {
    Title(String),
    Opening(String),
}

/// 開いたイベントストア。Swift 側は handle として持ち、ここの関数へ渡し返す。
#[uniffi::remote(Object)]
pub struct Escrow;

/// 一覧の1行（#6）。[`escrow_app::Listed`] は accessor しか公開しないので、値へ写す。
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Listed {
    pub published_on: String,
    pub headline: Headline,
    pub state: String,
    pub content_type: String,
}

impl From<&escrow_app::Listed> for Listed {
    fn from(listed: &escrow_app::Listed) -> Self {
        Self {
            published_on: listed.published_on(),
            headline: listed.headline().clone(),
            state: listed.state().to_owned(),
            content_type: listed.content_type().to_owned(),
        }
    }
}

/// 入口へ返す失敗。原因まで繋いだ1つの文。
///
/// 画面が出すのは理由の文だけなので、型を保ったまま運ばない（`escrow-gui` と同じ判断）。
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum FfiError {
    #[error("{0}")]
    Failed(String),
}

impl From<escrow_app::AppError> for FfiError {
    fn from(error: escrow_app::AppError) -> Self {
        Self::Failed(why(&error))
    }
}

/// 設定の言う場所でイベントストアを開く。
///
/// # Errors
///
/// [`escrow_app::App::open`] と同じ。
#[uniffi::export(async_runtime = "tokio")]
pub async fn open_escrow() -> Result<Arc<Escrow>, FfiError> {
    Ok(Arc::new(Escrow::open().await?))
}

/// 配信元の持ち主を全部。
///
/// # Errors
///
/// [`escrow_app::App::persons`] と同じ。
#[uniffi::export(async_runtime = "tokio")]
pub async fn persons(escrow: Arc<Escrow>) -> Result<Vec<Person>, FfiError> {
    Ok(escrow.persons().await?)
}

/// 選んだ持ち主の項目を、一覧の1行の形で新しい順に。
///
/// # Errors
///
/// [`escrow_app::App::items_of`] と同じ。
#[uniffi::export(async_runtime = "tokio")]
pub async fn items_of(escrow: Arc<Escrow>, person: PersonId) -> Result<Vec<Listed>, FfiError> {
    Ok(escrow
        .items_of(person)
        .await?
        .iter()
        .map(Listed::from)
        .collect())
}

/// 失敗の理由を、原因まで繋いで1つの文にする。
fn why(error: &dyn std::error::Error) -> String {
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
    use super::*;

    /// #30 の受け入れを、橋の向こうへ渡る値で見る。状態と種別は #1 の表の値そのまま、
    /// 新しいものが先、項目0件の持ち主は空。
    ///
    /// 仕込みは [`escrow_app::App::seeded`]（#82）。描いた結果は Swift 側の目視で、ここは
    /// Swift へ渡る直前の値のほう。
    #[tokio::test]
    async fn the_records_that_cross_the_bridge_carry_the_listing() {
        let media = tempfile::tempdir().unwrap();
        let escrow = Arc::new(Escrow::seeded(media.path()).await);

        let persons = persons(Arc::clone(&escrow)).await.unwrap();
        let names: Vec<&str> = persons.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["○○", "□□"]);

        let owner = persons.iter().find(|p| p.name == "○○").unwrap();
        let listed = items_of(Arc::clone(&escrow), owner.id).await.unwrap();
        assert_eq!(
            listed,
            [
                Listed {
                    published_on: "2026-03-01".to_owned(),
                    headline: Headline::Title("○○の雑談配信".to_owned()),
                    state: "holding".to_owned(),
                    content_type: "youtube_live".to_owned(),
                },
                Listed {
                    published_on: "2026-03-01".to_owned(),
                    headline: Headline::Opening("明日の配信は21時から。".to_owned()),
                    state: "kept".to_owned(),
                    content_type: "x_post".to_owned(),
                },
            ]
        );

        let nobody = persons.iter().find(|p| p.name == "□□").unwrap();
        assert!(items_of(escrow, nobody.id).await.unwrap().is_empty());
    }

    /// 失敗は原因まで繋いで1つの文にする。直す先を言うのは原因の側。
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
            why(&failure),
            "設定を読めない: 設定ファイルを TOML として読めない"
        );
    }
}

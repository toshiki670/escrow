//! 外部への引き渡し。#4 の `list` と `release`。
//!
//! CLI も GUI もここを呼ぶ。#4 が決めた形を組み立てる場所を1つにしておかないと、
//! 画面に出る値と外部が受け取る値がずれる。

use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::asset::{self, AssetKind};
use crate::item::{Item, ItemId};
use crate::state::{Event, ReleaseReference, State, StateName};
use crate::store::{Applied, Store, StoreError};
use crate::timestamp::Timestamp;

#[derive(Debug, Error)]
pub enum HandoverError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("項目 {0} が無い")]
    NoSuchItem(ItemId),
    #[error("項目 {id} は {state} なので引き渡せない")]
    NotReleasable { id: ItemId, state: StateName },
    #[error("読んだときから状態が動いている。やり直す")]
    Superseded,
    #[error("手元の実体を扱えない: {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// #4 が返す1件。**フィールドはちょうど9つ。**
///
/// `state_since` は返さない。期限の計算は escrow 側の仕事で、外部には関係しない（#4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Handover {
    /// `release` に渡す。
    pub id: i64,
    /// ノートの名前。`Post` では `null`。
    pub title: Option<String>,
    /// 投稿の本文。`Media` では `null`。
    pub body: Option<String>,
    /// 出典として記録する。
    pub url: String,
    pub content_type: String,
    /// 引き渡せる状態か判断する。
    pub state: String,
    /// ノート名の日付。
    pub published_at: String,
    /// 実体を読む・コピーする。X 投稿は画像を最大4枚持ち、ライブが途中で切れれば
    /// 動画も断片に分かれるので、単数では返せない（#4）。
    pub media_paths: Vec<String>,
    pub transcript_paths: Vec<String>,
}

/// 台帳の1行を、外部が受け取る形へ写す。
///
/// 実体のパスは `Item.id` から導出される値なので、`list` に含めてもコストがない。
/// だから #4 は `show` を持たない。
pub fn handover(item: &Item, media_dir: &Path) -> Result<Handover, HandoverError> {
    let dir = asset::item_dir(media_dir, item.id);
    let assets = asset::scan(media_dir, item.id).map_err(|source| HandoverError::Io {
        path: dir.clone(),
        source,
    })?;

    let paths_of = |kinds: &[AssetKind]| -> Vec<String> {
        assets
            .iter()
            .filter(|a| kinds.contains(&a.kind))
            .map(|a| dir.join(a.file_name()).to_string_lossy().into_owned())
            .collect()
    };

    let (title, body) = match &item.content {
        crate::content::Content::Media { title, .. } => (Some(title.clone()), None),
        crate::content::Content::Post { body, .. } => (None, Some(body.clone())),
    };

    Ok(Handover {
        id: item.id.get(),
        title,
        body,
        url: item.url.as_str().to_owned(),
        content_type: item.content_type().as_str().to_owned(),
        state: item.state.as_str().to_owned(),
        published_at: item.published_at.to_text(),
        media_paths: paths_of(&[AssetKind::Video, AssetKind::Audio, AssetKind::Image]),
        transcript_paths: paths_of(&[AssetKind::Transcript]),
    })
}

/// 外部が受け取り終えたことを伝える。
///
/// **DB を先に更新し、ファイルは後で消す**（#7）。逆順にすると、途中で落ちたときに
/// 「`kept` なのにメディアが無い」行が残り、#4 の「`kept` は引き渡しを待つ＝手元に
/// ある」という契約が破れる。DB を先にすれば、残るのは行と対応しない孤児ファイル
/// だけで、これは掃除できる。
pub async fn release(
    store: &Store,
    media_dir: &Path,
    id: ItemId,
    reference: Option<ReleaseReference>,
) -> Result<Handover, HandoverError> {
    let item = store.item(id).await?.ok_or(HandoverError::NoSuchItem(id))?;

    // #4 の「`holding` の項目も `list` に出るが、この場合 `release` は使えない」。
    // 遷移として弾かれるが、外へ返す理由をはっきりさせるためにここでも見る。
    if item.state != State::Kept {
        return Err(HandoverError::NotReleasable {
            id,
            state: item.state.name(),
        });
    }

    // 引き渡す中身は、消す前の姿で返す。受け取る側が何を持って行ったか分かる。
    let handed = handover(&item, media_dir)?;

    let applied = store
        .apply(
            id,
            &item.state,
            &Event::Released { reference },
            Timestamp::now(),
        )
        .await?;

    match applied {
        Applied::Written(_) => {}
        Applied::Superseded => return Err(HandoverError::Superseded),
    }

    // ここから先で落ちても、残るのは孤児ファイルだけ。
    asset::remove(media_dir, id).map_err(|source| HandoverError::Io {
        path: asset::item_dir(media_dir, id),
        source,
    })?;

    Ok(handed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Content, MediaType};
    use crate::source::{PersonId, SourceId};
    use crate::store::{NewItem, NewSource};
    use crate::url;
    use std::num::NonZeroU32;

    async fn seeded() -> (Store, SourceId) {
        let store = Store::open_in_memory().await.unwrap();
        let person: PersonId = store.add_person("○○").await.unwrap();
        let source = store
            .add_source(&NewSource {
                person_id: person,
                url: url::normalize_source(
                    "https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ",
                )
                .unwrap(),
                enabled: true,
                created_at: Timestamp::parse("2026-01-01T00:00:00+09:00").unwrap(),
                hold_days: None,
                discover_interval_minutes: NonZeroU32::new(15).unwrap(),
            })
            .await
            .unwrap();
        (store, source)
    }

    fn kept_item(source_id: SourceId) -> NewItem {
        NewItem {
            source_id,
            url: url::normalize_item("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                .unwrap()
                .0,
            published_at: Timestamp::parse("2026-03-01T20:00:00+09:00").unwrap(),
            state: State::Kept,
            state_since: Timestamp::parse("2026-03-01T22:30:00+09:00").unwrap(),
            content: Content::Media {
                media_type: MediaType::YoutubeLive,
                title: "○○の雑談配信".to_owned(),
            },
        }
    }

    fn put(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    #[tokio::test]
    async fn the_handover_has_exactly_the_nine_fields() {
        let (store, source) = seeded().await;
        let id = store.add_item(&kept_item(source)).await.unwrap();
        let media = tempfile::tempdir().unwrap();

        let item = store.item(id).await.unwrap().unwrap();
        let handed = handover(&item, media.path()).unwrap();
        let json: serde_json::Value = serde_json::to_value(&handed).unwrap();

        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "body",
                "content_type",
                "id",
                "media_paths",
                "published_at",
                "state",
                "title",
                "transcript_paths",
                "url",
            ]
        );
        // #4 は state_since を返さない。期限の計算は escrow 側の仕事。
        assert!(!json.as_object().unwrap().contains_key("state_since"));
    }

    /// `Media` は `body` が `null`、`Post` は `title` が `null`（#4）。
    #[tokio::test]
    async fn the_shape_decides_which_field_is_null() {
        let (store, source) = seeded().await;
        let media = tempfile::tempdir().unwrap();

        let id = store.add_item(&kept_item(source)).await.unwrap();
        let item = store.item(id).await.unwrap().unwrap();
        let handed = handover(&item, media.path()).unwrap();
        assert_eq!(handed.title.as_deref(), Some("○○の雑談配信"));
        assert_eq!(handed.body, None);

        let mut post = kept_item(source);
        post.url = url::normalize_item("https://x.com/jack/status/20")
            .unwrap()
            .0;
        post.content = Content::Post {
            body: "明日の配信は21時から。".to_owned(),
            in_reply_to: None,
            quoted: None,
        };
        let id = store.add_item(&post).await.unwrap();
        let item = store.item(id).await.unwrap().unwrap();
        let handed = handover(&item, media.path()).unwrap();
        assert_eq!(handed.title, None);
        assert_eq!(handed.body.as_deref(), Some("明日の配信は21時から。"));
    }

    #[tokio::test]
    async fn paths_are_split_by_what_they_are() {
        let (store, source) = seeded().await;
        let id = store.add_item(&kept_item(source)).await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let dir = asset::item_dir(media.path(), id);

        for name in [
            "video.1.mp4",
            "video.2.mp4",
            "image.1.jpg",
            "transcript.1.vtt",
        ] {
            put(&dir, name);
        }

        let item = store.item(id).await.unwrap().unwrap();
        let handed = handover(&item, media.path()).unwrap();

        assert_eq!(handed.media_paths.len(), 3, "動画2本と画像1枚");
        assert_eq!(handed.transcript_paths.len(), 1);
        assert!(handed.transcript_paths[0].ends_with("transcript.1.vtt"));
    }

    #[tokio::test]
    async fn releasing_updates_the_ledger_then_removes_the_files() {
        let (store, source) = seeded().await;
        let id = store.add_item(&kept_item(source)).await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let dir = asset::item_dir(media.path(), id);
        put(&dir, "video.1.mp4");
        put(&dir, "transcript.1.vtt");

        let handed = release(
            &store,
            media.path(),
            id,
            Some(ReleaseReference::new("Attachments/2026-03-01 ○○.mp4")),
        )
        .await
        .unwrap();

        // 返すのは消す前の姿。受け取る側が何を持って行ったか分かる。
        assert_eq!(handed.media_paths.len(), 1);
        assert_eq!(handed.transcript_paths.len(), 1);

        // 行は残り、参照が入る。
        let item = store.item(id).await.unwrap().unwrap();
        assert_eq!(
            item.state,
            State::Released {
                reference: Some(ReleaseReference::new("Attachments/2026-03-01 ○○.mp4")),
            }
        );
        // 実体は消える。
        assert!(!dir.exists());
    }

    /// #4 の決め事。`holding` は `list` に出るが `release` は使えない。
    #[tokio::test]
    async fn holding_cannot_be_released() {
        let (store, source) = seeded().await;
        let mut holding = kept_item(source);
        holding.state = State::Holding;
        let id = store.add_item(&holding).await.unwrap();
        let media = tempfile::tempdir().unwrap();
        let dir = asset::item_dir(media.path(), id);
        put(&dir, "video.1.mp4");

        assert!(matches!(
            release(&store, media.path(), id, None).await,
            Err(HandoverError::NotReleasable {
                state: StateName::Holding,
                ..
            })
        ));
        // 断られたら、実体はそのまま。
        assert!(dir.join("video.1.mp4").is_file());
    }

    #[tokio::test]
    async fn releasing_something_absent_is_refused() {
        let (store, _) = seeded().await;
        let media = tempfile::tempdir().unwrap();

        assert!(matches!(
            release(&store, media.path(), ItemId::new(999), None).await,
            Err(HandoverError::NoSuchItem(_))
        ));
    }
}

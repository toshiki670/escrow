//! Phase 4.2 の受け入れ（#7）。
//!
//! - 事象を追記して畳んだ結果と、投影の `item` が一致する
//! - 同じ `seq` を2回書くと弾かれる
//! - 期限は `acquired` の1回で確定し、そこから先は状態が運ぶ
//! - 沈黙は記録されない（#5 の非対称性）

use std::num::NonZeroU32;

use escrow_domain::content::{Content, ContentType, MediaType};
use escrow_domain::item::{Discovered, ItemId};
use escrow_domain::liveness::Presence;
use escrow_domain::source::{Monitoring, SourceId};
use escrow_domain::state::{
    Event, FailureReason, Hold, MediaPresence, ReleaseReference, State, StateName, TranscriptNeed,
};
use escrow_domain::timestamp::Timestamp;
use escrow_domain::url;
use escrow_ledger::{Ledger, LedgerError, NewSource, Seq};

fn at(text: &str) -> Timestamp {
    Timestamp::parse(text).expect("固定値")
}

fn one() -> NonZeroU32 {
    NonZeroU32::MIN
}

async fn seeded() -> (Ledger, SourceId) {
    let ledger = Ledger::open_in_memory().await.unwrap();
    let person = ledger.add_person("○○").await.unwrap();
    let source = ledger
        .add_source(&NewSource {
            person_id: person,
            url: url::normalize_source("https://www.youtube.com/channel/UCBR8-60-B28hp2BmDPdntcQ")
                .unwrap(),
            enabled: true,
            created_at: at("2026-01-01T00:00:00+09:00"),
            hold_days: Some(NonZeroU32::new(7).unwrap()),
            priority: one(),
            monitoring: Monitoring::Continuous,
        })
        .await
        .unwrap();
    (ledger, source)
}

fn a_live(source_id: SourceId, video_id: &str) -> Discovered {
    Discovered {
        source_id,
        url: url::normalize_item(&format!("https://www.youtube.com/watch?v={video_id}"))
            .unwrap()
            .0,
        published_at: at("2026-03-01T20:00:00+09:00"),
        scheduled_start_at: None,
        content: Content::Media {
            media_type: MediaType::YoutubeLive,
            title: "○○の雑談配信".to_owned(),
        },
        media: MediaPresence::Present,
    }
}

/// 投影と、ログを畳んだ結果が同じであること。
///
/// 同じトランザクションで書いているので、どの時点で読んでも一致する。
async fn agrees(ledger: &Ledger, id: ItemId) {
    let projected = ledger.item(id).await.unwrap().expect("投影に居る");
    let replayed = ledger.replay(id).await.unwrap().expect("ログに居る");
    assert_eq!(projected.item, replayed, "投影とログの畳み込みがずれている");
}

#[tokio::test]
async fn the_projection_follows_the_log_at_every_step() {
    let (ledger, source) = seeded().await;
    let id = ledger
        .discover(
            &a_live(source, "dQw4w9WgXcQ"),
            at("2026-03-01T20:05:00+09:00"),
        )
        .await
        .unwrap();

    agrees(&ledger, id).await;
    assert_eq!(
        ledger.item(id).await.unwrap().unwrap().item.state,
        State::Waiting,
        "取得する実体があるので waiting から始まる"
    );

    let deadline = at("2026-03-09T00:30:00+09:00");
    let steps: Vec<(Event, &str)> = vec![
        (Event::AcquisitionStarted, "2026-03-01T20:10:00+09:00"),
        (
            Event::Acquired {
                transcript: TranscriptNeed::Needed,
                hold: Hold::Until(deadline),
            },
            "2026-03-02T00:30:00+09:00",
        ),
        (Event::Transcribed, "2026-03-02T01:15:00+09:00"),
        (Event::SourceGone, "2026-03-05T09:00:00+09:00"),
        (
            Event::Released {
                reference: Some(ReleaseReference::new("Attachments/2026-03-01 ○○.mp4")),
            },
            "2026-03-06T21:00:00+09:00",
        ),
    ];

    let mut seq = Seq::FIRST;
    for (event, occurred_at) in steps {
        seq = ledger
            .append(id, seq, &event, at(occurred_at))
            .await
            .unwrap();
        agrees(&ledger, id).await;
    }

    let projected = ledger.item(id).await.unwrap().unwrap();
    assert_eq!(
        projected.item.state,
        State::Released {
            reference: Some(ReleaseReference::new("Attachments/2026-03-01 ○○.mp4")),
        }
    );
    assert_eq!(projected.seq.get(), 6, "誕生 + 5件");
}

/// 期限は `acquired` の1回で確定し、そこから先は状態が運ぶ（#1）。
#[tokio::test]
async fn the_deadline_is_fixed_once_and_carried_by_the_state() {
    let (ledger, source) = seeded().await;
    let id = ledger
        .discover(
            &a_live(source, "dQw4w9WgXcQ"),
            at("2026-03-01T20:05:00+09:00"),
        )
        .await
        .unwrap();

    let deadline = at("2026-03-09T00:30:00+09:00");
    let seq = ledger
        .append(
            id,
            Seq::FIRST,
            &Event::AcquisitionStarted,
            at("2026-03-01T20:10:00+09:00"),
        )
        .await
        .unwrap();
    let seq = ledger
        .append(
            id,
            seq,
            &Event::Acquired {
                transcript: TranscriptNeed::Needed,
                hold: Hold::Until(deadline),
            },
            at("2026-03-02T00:30:00+09:00"),
        )
        .await
        .unwrap();

    // 文字起こし中も期限を伴っている。
    assert_eq!(
        ledger.item(id).await.unwrap().unwrap().item.state,
        State::Transcribing {
            hold: Hold::Until(deadline)
        }
    );

    // `transcribed` は期限を運ばないのに、行き先は期限どおり。
    ledger
        .append(
            id,
            seq,
            &Event::Transcribed,
            at("2026-03-02T01:15:00+09:00"),
        )
        .await
        .unwrap();
    assert_eq!(
        ledger.item(id).await.unwrap().unwrap().item.state,
        State::Holding { until: deadline }
    );
    agrees(&ledger, id).await;
}

/// 読んでから書くまでに動いていれば弾かれる（#15）。
#[tokio::test]
async fn writing_from_a_stale_seq_is_refused() {
    let (ledger, source) = seeded().await;
    let id = ledger
        .discover(
            &a_live(source, "dQw4w9WgXcQ"),
            at("2026-03-01T20:05:00+09:00"),
        )
        .await
        .unwrap();

    // 2つの担い手が同じ姿を読んだ。
    let seen = ledger.item(id).await.unwrap().unwrap().seq;

    ledger
        .append(
            id,
            seen,
            &Event::AcquisitionStarted,
            at("2026-03-01T20:10:00+09:00"),
        )
        .await
        .unwrap();

    // 後から書く側は、既に埋まっている番号を指している。
    let second = ledger
        .append(id, seen, &Event::Deleted, at("2026-03-01T20:11:00+09:00"))
        .await;
    assert!(matches!(second, Err(LedgerError::Superseded)), "{second:?}");

    // 弾かれた側の事象はログにも投影にも入っていない。
    assert_eq!(
        ledger.item(id).await.unwrap().unwrap().item.state,
        State::Acquiring
    );
    assert_eq!(ledger.log(id).await.unwrap().unwrap().rest.len(), 1);
}

/// 状態を動かさない事象は、`state_since` を動かさない（#1）。
///
/// 確認できなかった回は行が増えないので、**沈黙が記録されないことが、そのまま
/// #5 の非対称性になる**。
#[tokio::test]
async fn confirming_presence_records_the_fact_without_moving_the_state() {
    let (ledger, source) = seeded().await;
    let id = ledger
        .discover(
            &a_live(source, "dQw4w9WgXcQ"),
            at("2026-03-01T20:05:00+09:00"),
        )
        .await
        .unwrap();

    let deadline = at("2026-03-09T00:30:00+09:00");
    let mut seq = Seq::FIRST;
    for (event, moment) in [
        (Event::AcquisitionStarted, "2026-03-01T20:10:00+09:00"),
        (
            Event::Acquired {
                transcript: TranscriptNeed::NotNeeded,
                hold: Hold::Until(deadline),
            },
            "2026-03-02T00:30:00+09:00",
        ),
    ] {
        seq = ledger.append(id, seq, &event, at(moment)).await.unwrap();
    }

    let became_holding = ledger.item(id).await.unwrap().unwrap().item.state_since;
    assert_eq!(became_holding, at("2026-03-02T00:30:00+09:00"));

    let confirmed = Presence::Present.confirmed().unwrap();
    for moment in [
        "2026-03-03T04:00:00+09:00",
        "2026-03-04T04:00:00+09:00",
        "2026-03-05T04:00:00+09:00",
    ] {
        seq = ledger
            .append(id, seq, &Event::PresenceConfirmed(confirmed), at(moment))
            .await
            .unwrap();
    }

    let projected = ledger.item(id).await.unwrap().unwrap();
    assert_eq!(
        projected.item.state,
        State::Holding { until: deadline },
        "確かめただけでは状態は動かない"
    );
    assert_eq!(
        projected.item.state_since, became_holding,
        "holding になった日時も動かない"
    );
    // 確かめた事実は3件とも残っている。
    assert_eq!(projected.seq.get(), 6);
    agrees(&ledger, id).await;
}

/// リトライ回数はカウンタではなく、ログから数える（#1）。
#[tokio::test]
async fn retries_are_counted_from_the_log() {
    let (ledger, source) = seeded().await;
    let id = ledger
        .discover(
            &a_live(source, "dQw4w9WgXcQ"),
            at("2026-03-01T20:05:00+09:00"),
        )
        .await
        .unwrap();

    let mut seq = ledger
        .append(
            id,
            Seq::FIRST,
            &Event::AcquisitionStarted,
            at("2026-03-01T20:10:00+09:00"),
        )
        .await
        .unwrap();

    for moment in ["2026-03-01T20:20:00+09:00", "2026-03-01T20:30:00+09:00"] {
        seq = ledger
            .append(
                id,
                seq,
                &Event::AttemptFailed {
                    reason: FailureReason::new("HTTP 403"),
                },
                at(moment),
            )
            .await
            .unwrap();
    }

    let log = ledger.log(id).await.unwrap().unwrap();
    assert_eq!(log.failures_since_the_state_moved().unwrap(), 2);

    // 状態が動けば数え直しになる。
    ledger
        .append(
            id,
            seq,
            &Event::Acquired {
                transcript: TranscriptNeed::NotNeeded,
                hold: Hold::None,
            },
            at("2026-03-02T00:30:00+09:00"),
        )
        .await
        .unwrap();
    let log = ledger.log(id).await.unwrap().unwrap();
    assert_eq!(log.failures_since_the_state_moved().unwrap(), 0);
    assert_eq!(
        log.replay().unwrap().state,
        State::Kept,
        "期限なしなら kept"
    );
}

/// 図に無い遷移は、書く前に弾かれる。
#[tokio::test]
async fn an_illegal_transition_never_reaches_the_log() {
    let (ledger, source) = seeded().await;
    let id = ledger
        .discover(
            &a_live(source, "dQw4w9WgXcQ"),
            at("2026-03-01T20:05:00+09:00"),
        )
        .await
        .unwrap();

    let refused = ledger
        .append(
            id,
            Seq::FIRST,
            &Event::Released { reference: None },
            at("2026-03-01T20:10:00+09:00"),
        )
        .await;
    assert!(
        matches!(refused, Err(LedgerError::IllegalTransition(_))),
        "{refused:?}"
    );
    assert_eq!(ledger.log(id).await.unwrap().unwrap().rest.len(), 0);
}

/// 同じ URL は二度起票できない（#1 の一意キー）。
#[tokio::test]
async fn the_same_url_cannot_be_discovered_twice() {
    let (ledger, source) = seeded().await;
    let discovered = a_live(source, "dQw4w9WgXcQ");

    ledger
        .discover(&discovered, at("2026-03-01T20:05:00+09:00"))
        .await
        .unwrap();
    let again = ledger
        .discover(&discovered, at("2026-03-01T20:06:00+09:00"))
        .await;
    assert!(again.is_err(), "{again:?}");
}

/// 実体を持たないものは `kept` から始まる（#1 の `[*]` から出る2本）。
#[tokio::test]
async fn a_text_only_post_starts_at_kept() {
    let (ledger, source) = seeded().await;
    let id = ledger
        .discover(
            &Discovered {
                source_id: source,
                url: url::normalize_item("https://x.com/jack/status/20")
                    .unwrap()
                    .0,
                published_at: at("2026-03-01T20:00:00+09:00"),
                scheduled_start_at: None,
                content: Content::Post {
                    body: "明日の配信は21時から。".to_owned(),
                    in_reply_to: None,
                    quoted: None,
                },
                media: MediaPresence::Absent,
            },
            at("2026-03-01T20:05:00+09:00"),
        )
        .await
        .unwrap();

    let projected = ledger.item(id).await.unwrap().unwrap();
    assert_eq!(projected.item.state, State::Kept);
    assert_eq!(projected.item.content_type(), ContentType::XPost);
    agrees(&ledger, id).await;
}

/// 状態で絞って読めること。エンジンはこれで拾う（#15）。
#[tokio::test]
async fn items_can_be_picked_up_by_state() {
    let (ledger, source) = seeded().await;
    for video in ["dQw4w9WgXcQ", "bLKBe3uMMRI", "wwJV2mo2US4"] {
        ledger
            .discover(&a_live(source, video), at("2026-03-01T20:05:00+09:00"))
            .await
            .unwrap();
    }

    let waiting = ledger.items_in_state(StateName::Waiting).await.unwrap();
    assert_eq!(waiting.len(), 3);

    ledger
        .append(
            waiting[0].item.id,
            waiting[0].seq,
            &Event::AcquisitionStarted,
            at("2026-03-01T20:10:00+09:00"),
        )
        .await
        .unwrap();

    assert_eq!(
        ledger
            .items_in_state(StateName::Waiting)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        ledger
            .items_in_state(StateName::Acquiring)
            .await
            .unwrap()
            .len(),
        1
    );
}

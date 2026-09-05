-- #1 の erDiagram のうち、**移行で管理するもの**。
--
-- 入るのは事象テーブルと、人が編集する設定テーブルだけ。投影（`item`）は入らない —
-- スキーマを変えたいときは移行を書かず `rebuild` で作り直すので、移行に混ぜると
-- いつの間にか投影に手作業の移行が当たって「捨てて作り直せる」が建前になる（#15）。
--
-- 日時は ISO 8601 の text、真偽値は integer で持つ（SQLite に専用の型が無いため）。
-- NULL を許すのは #1 の表に挙がっている列だけで、これ以外は NOT NULL。
--
-- 前進のみ。down migration は持たない（#7）。

CREATE TABLE person (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL
) STRICT;

CREATE TABLE source (
    id            INTEGER PRIMARY KEY,
    person_id     INTEGER NOT NULL REFERENCES person(id) ON DELETE CASCADE,
    url           TEXT    NOT NULL,
    enabled       INTEGER NOT NULL,
    -- 登録日時。これ以降の投稿を監視する。
    created_at    TEXT    NOT NULL,
    -- 検知の重み。予算の分け前を決める（#13）。間隔ではないので、実際の頻度は
    -- 予算と他の配信元との兼ね合いで決まる。
    priority      INTEGER NOT NULL,
    -- これから取得するものを何日預かるかの既定値。NULL は「捨てない」。
    -- 進行中の預かりには遡らない — 期限は取得完了の時点で確定する（#1）。
    hold_days     INTEGER,
    -- 監視の期間。両方 NULL なら区切らず継続して監視する。片方だけ埋まった行は
    -- 意味が決まっていないので、読み出しの parse が撥ねる。
    monitor_from  TEXT,
    monitor_until TEXT
) STRICT;

CREATE TABLE exclude (
    id           INTEGER PRIMARY KEY,
    -- NULL は全対象に効く共通条件。
    source_id    INTEGER REFERENCES source(id) ON DELETE CASCADE,
    content_type TEXT    NOT NULL,
    enabled      INTEGER NOT NULL
) STRICT;

-- 唯一の真実。追記のみで、書き換えも削除もしない（#15）。
--
-- `item_id` は投影を**参照しない**。投影は捨てて作り直せるものなので、真実の側が
-- そこへ外部キーを張ると、`rebuild` の DROP が連鎖してログごと消える。項目の同一性は
-- このテーブルが持ち、番号もここから採る。
--
-- 代わりに `source_id` を全ての行が持ち、削除の連鎖をここで受ける（#1 の
-- 「`SOURCE` を消すと、その `ITEM` と `ITEM_EVENT` も消える」）。値は誕生の時点で
-- 決まって動かないので、行ごとに持っても食い違わない。
CREATE TABLE item_event (
    id          INTEGER PRIMARY KEY,
    item_id     INTEGER NOT NULL,
    source_id   INTEGER NOT NULL REFERENCES source(id) ON DELETE CASCADE,
    -- 項目の中での通し番号。1 から始まり、1 は必ず discovered。
    seq         INTEGER NOT NULL,
    -- 事象の判別子。対応の保証は Rust の enum で、CHECK 制約は置かない（#1）。
    kind        TEXT    NOT NULL,
    occurred_at TEXT    NOT NULL,

    -- ここから下は「その kind が運ばないなら NULL」。
    -- discovered — 項目の中身を丸ごと運ぶ。これが無いと投影を作り直せない。
    url                TEXT,
    content_type       TEXT,
    published_at       TEXT,
    scheduled_start_at TEXT,
    title              TEXT,
    body               TEXT,
    in_reply_to_url    TEXT,
    quoted_url         TEXT,
    media_present      INTEGER,
    -- acquired — 預かりの期限そのもの。
    transcript_needed  INTEGER,
    hold_until         TEXT,
    -- released
    release_reference  TEXT,
    -- attempt_failed
    failure_reason     TEXT
) STRICT;

-- #1 の索引表。順序と競合検知を兼ねる。2つのプロセスが同じ seq を書こうとすれば
-- 片方が落ちる。「毎回 WHERE 句を書く」規律が、忘れられない制約になる（#15）。
CREATE UNIQUE INDEX item_event_seq ON item_event(item_id, seq);

-- #1 の索引表。同じ配信元を二度登録できないようにする。
CREATE UNIQUE INDEX source_url ON source(url);

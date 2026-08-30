-- #1 の erDiagram をそのまま落としたもの。
--
-- 日時は ISO 8601 の text、真偽値は integer で持つ（SQLite に専用の型が無いため）。
-- NULL を許すのは #1 の表に挙がっている列だけで、これ以外は NOT NULL。
--
-- subtype は single-table inheritance。「Media に body は無い」を保証するのは
-- Rust の enum であって DB ではないので、CHECK 制約は置かない（#1）。

CREATE TABLE person (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL
) STRICT;

CREATE TABLE source (
    id                        INTEGER PRIMARY KEY,
    person_id                 INTEGER NOT NULL REFERENCES person(id) ON DELETE CASCADE,
    url                       TEXT    NOT NULL,
    enabled                   INTEGER NOT NULL,
    -- 登録日時。これ以降の投稿を監視する。
    created_at                TEXT    NOT NULL,
    -- 預かる日数。NULL は「捨てない」。
    hold_days                 INTEGER,
    discover_interval_minutes INTEGER NOT NULL
) STRICT;

CREATE TABLE exclude (
    id           INTEGER PRIMARY KEY,
    -- NULL は全対象に効く共通条件。
    source_id    INTEGER REFERENCES source(id) ON DELETE CASCADE,
    content_type TEXT    NOT NULL,
    enabled      INTEGER NOT NULL
) STRICT;

CREATE TABLE item (
    id              INTEGER PRIMARY KEY,
    source_id       INTEGER NOT NULL REFERENCES source(id) ON DELETE CASCADE,
    -- 正規化した URL。項目の一意キー。
    url             TEXT    NOT NULL,
    content_type    TEXT    NOT NULL,
    published_at    TEXT    NOT NULL,
    state           TEXT    NOT NULL,
    state_since     TEXT    NOT NULL,
    -- ここから下が #1 の「NULL を許す」列。
    title           TEXT,  -- subtype が Post なら NULL
    body            TEXT,  -- subtype が Media なら NULL
    in_reply_to_url TEXT,  -- 返信ではない（Media は常に NULL）
    quoted_url      TEXT,  -- 引用ではない（Media は常に NULL）
    -- 外部が保存した先への参照。解釈せず保管する。まだ引き渡していなければ NULL。
    release_reference TEXT
) STRICT;

-- #1 の索引表。
CREATE UNIQUE INDEX item_url ON item(url);
-- 同じ配信元を二度登録できないようにする。item.url がグローバルに一意なので、
-- 二個目の配信元は同じ項目を入れられず、検知が毎回空振りする。
CREATE UNIQUE INDEX source_url ON source(url);
CREATE INDEX item_state ON item(state);
CREATE INDEX item_source_id ON item(source_id);

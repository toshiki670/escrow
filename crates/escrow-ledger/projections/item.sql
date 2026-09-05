-- #1 の erDiagram の ITEM。**投影**であって真実ではない（#15）。
--
-- `migrations/` に置かないのは、スキーマを変えたいときに移行ではなく `rebuild` を
-- 走らせるため。`sqlx::migrate!` は1ディレクトリ・1つの `_sqlx_migrations` で版を
-- 管理するので、事象と投影でディレクトリを割ることもできない。
--
-- このファイルが投影の DDL の唯一の写しで、`rebuild` は DROP のあとこれを流す。
-- クローン直後の開発用 DB もこれを流して作る（`.cargo/config.toml`）。
--
-- 列は #1 の erDiagram のまま。索引もそのままなので、読み出しのクエリと性能は
-- 事象ログを入れる前と変わらない。

CREATE TABLE IF NOT EXISTS item (
    id              INTEGER PRIMARY KEY,
    source_id       INTEGER NOT NULL REFERENCES source(id) ON DELETE CASCADE,
    -- 正規化した URL。項目の一意キー。
    url             TEXT    NOT NULL,
    content_type    TEXT    NOT NULL,
    published_at    TEXT    NOT NULL,
    -- ここから2つは導出列。state は事象を畳んだ結果、state_since は
    -- **状態を変えた**最後の事象の occurred_at（自己ループでは動かない）。
    state           TEXT    NOT NULL,
    state_since     TEXT    NOT NULL,
    -- ここから下が #1 の「NULL を許す」列。
    -- 配信の開始予定時刻。予約枠でなければ NULL。
    scheduled_start_at TEXT,
    -- 預かりの期限。預かりを通らない、またはまだ取得していなければ NULL。
    -- いまの状態が伴っている値の写しで、holding を抜ければ NULL に戻る。
    hold_until      TEXT,
    title           TEXT,  -- subtype が Post なら NULL
    body            TEXT,  -- subtype が Media なら NULL
    in_reply_to_url TEXT,  -- 返信ではない（Media は常に NULL）
    quoted_url      TEXT,  -- 引用ではない（Media は常に NULL）
    -- 外部が保存した先への参照。解釈せず保管する。まだ引き渡していなければ NULL。
    release_reference TEXT
) STRICT;

-- #1 の索引表。
CREATE UNIQUE INDEX IF NOT EXISTS item_url ON item(url);
CREATE INDEX IF NOT EXISTS item_state ON item(state);
CREATE INDEX IF NOT EXISTS item_source_id ON item(source_id);

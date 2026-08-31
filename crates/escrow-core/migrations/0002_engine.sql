-- エンジン（#7 の Phase 4）が要る2列。どちらも #1 の erDiagram へ足した。

-- 取得・文字起こしが連続して落ちた回数。`acquire.max_retries` と突き合わせる。
--
-- 現在の状態でこれまで何回落ちたかを数えるので、状態が動いたら 0 に戻る
-- （`Store::apply` が書き戻す）。状態と対でしか意味を持たない値だが、
-- `release_reference` と違って「その状態なら必ず在る」ので NOT NULL で持てる。
ALTER TABLE item ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;

-- 最後に検知が通った日時。NULL は「まだ一度も通っていない」。
--
-- 2つの用途を兼ねる。次にいつ回すか（`discover_interval_minutes` を足す）と、
-- どこまで遡るか（ここから `Discover::discover` の `since` を出す）。
ALTER TABLE source ADD COLUMN last_discovered_at TEXT;

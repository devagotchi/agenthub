-- Keep the request that caused an audit event without rewriting historical hashes.
ALTER TABLE devagotchi.board_events ADD COLUMN IF NOT EXISTS request_id TEXT;

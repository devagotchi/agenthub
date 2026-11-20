ALTER TABLE devagotchi.board_settings
    ADD COLUMN IF NOT EXISTS version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0);

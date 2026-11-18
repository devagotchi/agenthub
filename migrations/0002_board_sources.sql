-- Where an imported ticket came from: the source document and item its creator named, once
-- per board. (path, item) is the ticket's import identity there; the row is written with the
-- ticket, in its transaction, and never changed. request_sha256 is the digest of the inputs
-- the ticket was made with (version 1): a retry with the same inputs returns the ticket, with
-- other inputs it is refused as source_conflict.
CREATE TABLE IF NOT EXISTS devagotchi.board_sources (
  tenant_id TEXT NOT NULL,
  board_id TEXT NOT NULL,
  ticket TEXT NOT NULL,
  path TEXT NOT NULL,
  item TEXT NOT NULL,
  revision TEXT,
  content_sha256 TEXT,
  recorded_by TEXT NOT NULL,
  recorded_at BIGINT NOT NULL,
  request_sha256 TEXT NOT NULL,
  PRIMARY KEY (tenant_id, board_id, ticket),
  UNIQUE (tenant_id, board_id, path, item),
  FOREIGN KEY (tenant_id, board_id, ticket)
    REFERENCES devagotchi.board_tickets (tenant_id, board_id, id)
);

-- Never changed: no update (an INSERT ... ON CONFLICT DO UPDATE is one) and no delete.
CREATE OR REPLACE TRIGGER board_sources_append_only BEFORE UPDATE OR DELETE
  ON devagotchi.board_sources FOR EACH ROW EXECUTE FUNCTION devagotchi.board_append_only();

-- As every board table: a row only inside a transaction scoped to its tenant; the host's role
-- (`devagotchi_hub`, when there is one) may read and write it.
DO $$
BEGIN
  ALTER TABLE devagotchi.board_sources ENABLE ROW LEVEL SECURITY;
  ALTER TABLE devagotchi.board_sources FORCE ROW LEVEL SECURITY;
  IF NOT EXISTS (
    SELECT 1 FROM pg_policies
    WHERE schemaname = 'devagotchi' AND tablename = 'board_sources' AND policyname = 'tenant_only'
  ) THEN
    CREATE POLICY tenant_only ON devagotchi.board_sources
      USING (tenant_id = current_setting('devagotchi.tenant', true))
      WITH CHECK (tenant_id = current_setting('devagotchi.tenant', true));
  END IF;
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'devagotchi_hub') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON devagotchi.board_sources TO devagotchi_hub;
  END IF;
END $$;

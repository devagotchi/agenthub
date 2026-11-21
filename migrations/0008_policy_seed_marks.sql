-- Independent source policy families are applied once, never restoring operator edits.
CREATE TABLE devagotchi.board_policy_seed_marks (
  tenant_id TEXT NOT NULL,
  family TEXT NOT NULL,
  PRIMARY KEY (tenant_id, family)
);
ALTER TABLE devagotchi.board_policy_seed_marks ENABLE ROW LEVEL SECURITY;
ALTER TABLE devagotchi.board_policy_seed_marks FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_only ON devagotchi.board_policy_seed_marks
  USING (tenant_id=current_setting('devagotchi.tenant',true))
  WITH CHECK (tenant_id=current_setting('devagotchi.tenant',true));
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='devagotchi_hub') THEN
    GRANT SELECT, INSERT, UPDATE, DELETE ON devagotchi.board_policy_seed_marks TO devagotchi_hub;
  END IF;
END $$;

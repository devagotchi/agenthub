-- Shared definitions in each board owner's namespace. The publishing board retains
-- their audit history; bindings point at one authoritative definition. Add-only.
CREATE TABLE IF NOT EXISTS devagotchi.board_workflows (
  tenant_id TEXT NOT NULL,
  owner_id TEXT NOT NULL,
  id TEXT NOT NULL,
  source_board TEXT NOT NULL,
  version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
  value TEXT NOT NULL,
  PRIMARY KEY (tenant_id, owner_id, id),
  FOREIGN KEY (tenant_id, source_board) REFERENCES devagotchi.board_projects (tenant_id, id)
);
CREATE TABLE IF NOT EXISTS devagotchi.board_workflow_bindings (
  tenant_id TEXT NOT NULL,
  board_id TEXT NOT NULL,
  owner_id TEXT NOT NULL,
  workflow_id TEXT NOT NULL,
  PRIMARY KEY (tenant_id, board_id),
  FOREIGN KEY (tenant_id, board_id) REFERENCES devagotchi.board_projects (tenant_id, id),
  FOREIGN KEY (tenant_id, owner_id, workflow_id)
    REFERENCES devagotchi.board_workflows (tenant_id, owner_id, id)
);
CREATE INDEX IF NOT EXISTS board_workflow_uses
  ON devagotchi.board_workflow_bindings (tenant_id, owner_id, workflow_id);
DO $$
DECLARE tab TEXT;
BEGIN
  FOREACH tab IN ARRAY ARRAY['board_workflows','board_workflow_bindings'] LOOP
    EXECUTE format('ALTER TABLE devagotchi.%I ENABLE ROW LEVEL SECURITY', tab);
    EXECUTE format('ALTER TABLE devagotchi.%I FORCE ROW LEVEL SECURITY', tab);
    IF NOT EXISTS (SELECT 1 FROM pg_policies
        WHERE schemaname='devagotchi' AND tablename=tab AND policyname='tenant_only') THEN
      EXECUTE format('CREATE POLICY tenant_only ON devagotchi.%I
        USING (tenant_id=current_setting(''devagotchi.tenant'',true))
        WITH CHECK (tenant_id=current_setting(''devagotchi.tenant'',true))', tab);
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='devagotchi_hub') THEN
      EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON devagotchi.%I TO devagotchi_hub', tab);
    END IF;
  END LOOP;
END $$;

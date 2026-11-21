-- Versioned principal kinds, roles and rules from the October source. Tenant scoped;
-- identity/credential rows remain host owned. Mutations join the caller's audit transaction.
CREATE TABLE IF NOT EXISTS devagotchi.board_principal_settings (
  tenant_id TEXT PRIMARY KEY,
  version BIGINT NOT NULL CHECK (version > 0),
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS devagotchi.board_roles (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  description TEXT NOT NULL,
  version BIGINT NOT NULL CHECK (version > 0),
  PRIMARY KEY (tenant_id, id)
);
CREATE TABLE IF NOT EXISTS devagotchi.board_policy_rules (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  version BIGINT NOT NULL CHECK (version > 0),
  value TEXT NOT NULL,
  PRIMARY KEY (tenant_id, id)
);
DO $$
DECLARE tab TEXT;
BEGIN
  FOREACH tab IN ARRAY ARRAY['board_principal_settings','board_roles','board_policy_rules'] LOOP
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

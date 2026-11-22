-- A removed project retains its immutable identity, history and request receipts.
-- A later project using its name is a different board, not a revived generation.
ALTER TABLE devagotchi.board_projects ADD COLUMN IF NOT EXISTS retired_at BIGINT;
ALTER TABLE devagotchi.board_projects ADD COLUMN IF NOT EXISTS display_name TEXT NOT NULL DEFAULT '';
ALTER TABLE devagotchi.board_projects ADD COLUMN IF NOT EXISTS version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0);
UPDATE devagotchi.board_projects SET display_name=coalesce(project_name,id) WHERE display_name='';
DROP INDEX IF EXISTS devagotchi.board_projects_owner_name;
CREATE UNIQUE INDEX board_projects_owner_name ON devagotchi.board_projects
  (tenant_id,owner_id,project_name) WHERE retired_at IS NULL;

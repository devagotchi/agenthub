-- A credential-bound principal's live work may span multiple boards. The tenant
-- remains part of both the lookup and RLS; board-local aliases retain board limits.
CREATE INDEX IF NOT EXISTS board_tickets_principal_leases
  ON devagotchi.board_tickets (tenant_id, holder, lease_expires_at)
  WHERE holder IS NOT NULL;

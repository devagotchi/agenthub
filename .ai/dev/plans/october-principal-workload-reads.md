# Read the original agent roster's real workload

Source: preserved AgentHub 46419c6, web/src/pages/Agents.tsx (current claims beside
the principal's workload limit) and server/internal/store/lease.go. Branch:
increment-287/principal-workload-reads. Continues the principal capacity prerequisite
merged in PR15 at 97c84f2; it does not complete that historical source.

Objective: give the host one domain-owned read for a verified principal's exact live
workload and a bounded page of claims on boards the host has authorized. Count across
the tenant, even when a board is no longer visible; never expose hidden board or task
details. Use one database-clock instant and transaction snapshot, as claims do.

Non-goals: authentication/profile CRUD, editable roles/policy, browser roster and
OAuth bindings. These remain the host integration; this library creates no auth store
or token format. No SQLite or second board query implementation in the host.

Tasks/dependencies: extend hosted registry with explicit bounded inputs, deterministic
keyset pagination and complete counts; test visible/hidden projects, tenant/principal
isolation, expiry/release/completion, invalid inputs, read failures and unchanged events;
validate the parent workspace and standalone PostgreSQL CI; merge on green and verify
the reviewed tree before deleting this feature branch. Adopt its gitlink with host work.

Acceptance: counts match the live-lease capacity rule, independent of pagination and
visible board filtering. Hidden task titles/ids/board names never appear. Empty visible
sets show no claims; stale or arbitrary cursors cannot broaden access. Page boundaries
use board id and ticket id, including colliding ticket ids on different boards. Reads
emit no writes or events. Storage errors remain errors, never zero workload.

Risks: caller-provided identity or board access, cross-tenant disclosure, expired leases
shown as active, bounded pages mistaken for totals. Document the host's authenticated
identity/visibility responsibilities; return the global count separately from a bounded
visible page and continuation cursor. Real PostgreSQL tests cover these distinctions.

Implementation: complete for the read prerequisite. Three focused PostgreSQL tests and
all 921 parent-workspace tests passed with PostgreSQL required (114 AgentHub, 514 hub).
Final binary build, Windows process E2E, MCP protocol, generated contracts, AI/fmt/layout
and diff checks passed. Dependency manifests/lockfile are unchanged from the passing audit.
An initial failure-injection test assumed the host database role existed in the library
fixture; it now injects a schema read failure in its own disposable database. The failed
log is retained, separate from the passing rerun. Standalone Linux PostgreSQL CI remains
the merge gate. Push/PR/main: pending. Host profile/credential integration remains open.
DELIVERY INCOMPLETE.

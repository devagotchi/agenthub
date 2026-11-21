# Restore a principal's workload limit across projects

Source: oldest preserved AgentHub branch46419c6, store/lease.go checkWIP. It counts
all live leases for an authenticated principal across projects. Current hosted leases
count only the current board, even when Actor.wip_limit supplies a principal limit.
Branch: increment-286/principal-workload-limits.

Objective: an explicit principal limit applies across every board in its tenant, in the
same serializable transaction that acquires the lease. Claim and claim-next share the
check. Already-held retries preserve their fence; expired/released/completed work frees
capacity; zero remains unlimited. A tenant's principal never counts another tenant's work.

The host must supply an authenticated actor id unique across its tenant when supplying
this limit. Existing account-owned seat aliases without a principal limit retain their
board default; they are not silently treated as globally authenticated principals.
Credential binding, profile management, configured roles/policy and the roster are the
next host integration, not claimed delivered by this backend change. No new auth store,
scheduler or SQLite path. Existing SERIALIZABLE transactions provide cross-board race
protection; an index supports the live-holder count.

Tasks: document the explicit-limit contract; extend the shared claim check; add the index;
prove cross-board lifecycle, claim-next, isolation, unchanged alias defaults and concurrent
claims through independent board instances; run PostgreSQL tests, parent compatibility,
build/fmt/layout and applicable delivery checks; green backend CI, merge and delete the
feature branch before adopting its gitlink in the host.

Acceptance: a principal at limit1 cannot take another board's task, including claim-next;
refusal changes no ticket/fence/audit/outbox; release, completion and expiry free capacity;
six simultaneous claims on distinct boards produce exactly one winner at limit1; other
principals and tenants remain independent; zero and board-default behavior remain explicit.

Risks: counting caller-chosen aliases globally, cross-tenant leakage, and write skew across
board locks. Scope the new behavior to explicit authenticated principal limits, keep the
tenant predicate, and exercise real concurrent transactions. This is a prerequisite for
the original agent feature, not completion of the historical branch.

Implementation: complete for this backend prerequisite. Validation: three focused real-PG
tests and all918 parent-workspace tests passed, including111 AgentHub and514 hub tests;
final binary build, Windows process E2E, protocol, contracts, AI/fmt/layout/diff passed.
Dependency manifests/lockfile are unchanged from the passing audit. An initial test-temp
location inside the checkout was correctly refused by the AI-state tests; rerun uses D:
outside the checkout. A stale migration-history assertion was updated to include version5.
Those failed runs are retained and excluded from passing evidence. Backend Linux PostgreSQL
CI still must pass. Push/PR/main: pending. Host credential/profile integration is separate.
DELIVERY INCOMPLETE.

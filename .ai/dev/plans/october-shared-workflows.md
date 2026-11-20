# Restore shared workflows and project bindings

Continue oldest source `46419c6`: `a703761` admin_config.go PutWorkflow/PutProject,
admin_misc.go DeleteConfig, and `dcca3d3` Workflows.tsx selection and used-by list.
PR13 backend and parent PR277 are merged green; their feature branches are deleted.
Parent main is c52e773. Its official image is deployed; no-JS configuration, existing-user,
health and restart-persistence Docker acceptance passed.

Branch: increment-284/shared-workflow-catalog. Extend AgentHub's existing PostgreSQL
workflow owner, without a separate task service. A shared definition belongs to the
same tenant and owner namespace as its boards. The host must require authority over
that namespace for catalog management; access to one shared project is insufficient.
Existing private per-board definitions remain private until explicitly published/bound.

Publish a board's workflow into its owner's catalog and bind that board. Bind another
board to a named version; detach into an independent copy of the current definition.
Edits validate every using board, lock their rows, advance their configuration versions,
and publish their events in one transaction. Stale catalog/board versions refuse writes.
A definition still in use cannot be deleted. Direct per-board workflow edits refuse
while bound, so they cannot silently overwrite a shared definition or detach a board.

The publishing board remains the catalog entry's audit anchor, even after detachment.
Catalog create/edit/delete events use its existing append-only chain/outbox; binding
changes and effective workflow changes also appear on each affected board. The anchor
is retained by a foreign key. No hidden board or second event service is introduced.
Catalog definitions are authoritative; detached board JSON is only used when unbound.

Milestones: additive scoped schema and types; catalog/binding transactions and sync face;
PostgreSQL tests; applicable workspace validation; backend green CI/merge/deletion;
parent forms with owner authority, labelled selection/editing and used-by list; browser,
CI, merge/deletion and Docker acceptance. Backend-only delivery does not close this feature.

Acceptance: two boards observe one edited definition and real task lifecycle behavior;
one board's incompatible used state rolls back the whole edit; races have one versioned
winner; stale binding/detach fails; unbinding preserves the latest definition; bound
direct edits fail; in-use deletion fails; another owner/tenant cannot discover or bind
the definition; human-only mutations, no-op behavior, real storage failure rollback,
chain/outbox verification and upgrade preservation. Exercise all default tests with
required PostgreSQL, build, dependency checks, E2E/protocol/contracts/AI/fmt before push.

Risks: cross-board lock ordering, accidental authority expansion, lost drafts on catalog
updates, and event atomicity. Catalog mutations take one namespace lock before board
locks; existing single-board operations never acquire the namespace lock afterward.
No task state is reinterpreted or removed. No history or original source branch is deleted.
Principals, policies, generic request replay, retention and other original gaps stay open.

Implementation: backend implemented; parent catalog UI pending. Focused shared workflow
tests and all 108 backend tests passed with required PostgreSQL, including restricted-role
RLS and rollback after a preceding board write. Application build and dependency checks
passed. Full integration initially exposed an exhausted disposable-test disk, then a
parent source scanner that assumed flat source directories. Both causes were corrected;
the parent's recursive scanner and new refusal hints passed their three focused tests.
The final sequential workspace run passed all 911 tests (including desktop), 510 hub and
108 AgentHub tests with required PostgreSQL. An intervening build failed when a concurrent
policy command locked the Windows executable; that failed run is retained and excluded.
Final binary build, Windows process E2E, MCP protocol, generated contracts, dependency
audit, AI contracts, formatting and diff checks passed. The first E2E run exposed a
Unix-only process fixture; the parent harness now uses its existing Python interpreter
and the complete rerun passed. Linux-only IPC/MCP smoke, optional analytics process
proof and deployment remain separate parent CI gates, not local Windows claims.
Push, PR and main: pending. Parent integration and harness fixes must accompany adoption
of this backend revision. Browser delivery remains pending.
DELIVERY INCOMPLETE.

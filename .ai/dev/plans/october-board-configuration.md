# Restore October board configuration

Continues oldest source `46419c6`: `a703761` admin_config.go/settings/workflow and
`dcca3d3` Settings UI. Parent PR276 merged as 9c6dc6e after all 18 checks passed;
its feature branches were deleted before this branch began. Its Docker image is building.

Backend branch `increment-283/versioned-board-configuration` extends the existing
HostedBoard settings/workflow owner. Add optimistic versions to existing configuration
rows; every existing configuration writer advances them only when it writes. Read settings
and workflow together. Human-only versioned edits retain independently managed standing
approvals and reject stale versions in the same transaction as the update and audit event.
Keep the existing project-anchor MCP behavior. No second settings store or SQL in web code.

Validate lease bounds, nonnegative WIP/expiry limits, repo/environments, workflow identifiers,
labels, unique states/lanes/transitions and runnable lifecycle relationships. A workflow must
retain every used state and its kind, so changing configuration cannot reinterpret existing
tasks or orphan held work. Failures leave configuration, versions and audit unchanged.

Parent increment-283 will expose board settings and a form-based workflow editor to people
with configuration authority, using the existing browser app and localized copy. Forms show
states, work stages and manual transitions with labels/selects; raw JSON is not the primary
editing interface. Changes require this hub's origin, current authentication and board
authority. Account separation and hosted-role rules remain enforced. Agent principals,
cross-project WIP and editable policy remain separate open source gaps.
This increment edits each board's existing workflow. The original shared workflow registry
and project-to-workflow rebinding are still separate parity gaps; no registry parity is claimed.

Backend acceptance: PostgreSQL stale/concurrent edits, no-op behavior, version advancement
by existing writers, settings isolation, preservation of standing approvals, invalid input
rollback, actual lease/default effects, workflow changes with existing/held tasks, audit and
tenant/board isolation. Run the complete backend suite with PostgreSQL required, publish,
wait for green CI, merge and delete its branch before starting the parent implementation.
Parent acceptance includes HTTP authority/CSRF/storage/conflict tests, real browser forms,
full applicable suite and all CI checks before merge/delete, then Docker acceptance.

Migration adds only configuration row versions. No credentials, data deletion, deployment
promotion or historical-source completion claim. User authorization covers this sequential
restoration and validated merges; no agents are delegated.

Backend implementation complete. Local validation passed: all 102 AgentHub tests and all
898 parent default-workspace tests (PostgreSQL required, four test threads), application
build, Windows end-to-end tests against a fresh database, stdio protocol, generated contracts,
AI contracts, formatting/diff checks and dependency audit. The migration test upgrades
existing values from schema version 2; real claims enforce changed defaults and WIP limits;
concurrent saves have one winner. Linux PostgreSQL execution remains the PR CI gate.
Parent browser forms and their acceptance are still pending. Push/PR/main: pending.
DELIVERY INCOMPLETE until the backend is green and merged; overall source absorption remains open.

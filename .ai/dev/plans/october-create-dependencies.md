# October baseline: atomic task dependencies

Source: predecessor `agent-hub` commits `a703761` (Postgres task creation) and `dcca3d3`
(`web/src/pages/NewTask.tsx` sends dependencies with the new task).
Branch: `increment-278/atomic-ticket-dependencies`, from AgentHub main `f4af7f9`.
Parent integration: Devagotchi's `increment-278/absorb-agenthub-board-creation`, tracked in
`.ai/dev/plans/agenthub-factory.md` there. This packet does not complete the older branch.

Implement `NewTicket.depends_on` through the existing Postgres create transaction. Validate
every referenced ticket in the same tenant and board, deduplicate dependencies, include
them in the creation event and source replay digest. A bad dependency must leave no ticket,
dependency, child sequence increment, source row or event behind. No separate store.

Acceptance: real Postgres tests for atomic creation, missing/foreign dependency refusal,
blocking of claims, no duplicate edges, and same-source replay/conflict behavior. Existing
source digest fixtures without dependencies must stay valid. Parent verifies browser/MCP.

Non-goals: no new schema, SQLite, principal-policy replacement, or claim that the remaining
October interface and backend features have been absorbed.

Implementation: dependency sets commit with creation, source replay compares their normalized
set, and CLI `new --depends-on ID` (repeatable) forwards them to standalone and bound boards.
Validation: all 90 AgentHub library tests passed on isolated PostgreSQL on 2026-11-20,
including three new dependency transaction/isolation/replay tests. Parent HTTP creation
and MCP consistency tests passed; parent CLI process test and full delivery checks pending.
Delivery: local implementation verified for this library; push, PR and main pending.

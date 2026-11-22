# Oldest-source absorption: board behavior

Source: preserved Go AgentHub `46419c697964016eb547c60416a8f2f5e854a95f`, first of the
five preserved sources. Local branch: `increment-288/absorb-october-baseline`. Parent
integration is devagotchi/devagotchi's plan `.ai/dev/plans/absorb-preserved-sources.md`
and its immutable `.ai/dev/plans/absorption/` file/commit ledger.

Publication correction, 2026-11-22: the operator directs publishing the authored
integration and maintaining a visible roadmap toward green main. The parent plan records
this scheduling correction. Source completion remains oldest-first; this integration does
not close S1 or authorize deleting historical sources. Hosted data remains PostgreSQL.

Initial authored corrections from source store/edit.go, notes.go, hint.go and ops.go:

- Metadata updates carry lease fences and progress through the sync/hosted API and
  both CLI faces. A live holder needs its fence; supplied stale fences refuse even
  after reclaim; a different agent cannot overwrite the holder. Version guards remain.
- Notes reject supplied stale fences after the writer loses the lease. Configurable
  note kinds and heartbeat hints restore source settings; no configuration change
  bypasses the lease lifecycle or deletes old data.
- Handoff retains last-worker attribution. Completion with a missing active stage
  refuses instead of reporting success. Human force-release retains a ticket note.
  Human delegation cannot be used by an agent as an unfenced handoff.
- New source-specific tests cover atomic refusals, reclaim, metadata/progress, CLI
  fence caching, stored settings, distinct review and damaged workflow completion.

Parent surfaces include MCP schema/handlers, browser progress and versioned settings
forms, plus matching HTTP/MCP tests. Policy/roles/registered identities, general replay,
remaining settings and other source behavior are still open in the parent packets.
This work does not close the full source or authorize deletion of its historical refs.

Earlier checkpoint validation (superseded by the receipt below): nine focused checks passed before the lifecycle additions.
The subsequent library run passed 123 tests; refusal-help coverage found the new
not_completable code needed an explicit CLI next step. That mapping and its MCP counterpart
are now authored, and the affected refusal-help test passed. All 57 parent AgentHub-surface
tests then passed with PostgreSQL required. No full-workspace or CI claim is made for
this unfinished source batch.

Acceptance before source closure: every original file/commit mapped; domain and all
relevant surfaces implemented; source-specific checks, complete integration checks and
real platform/browser/Docker acceptance; green reviewed-head and main CI for each complete source; verified merge and only then branch deletion.

## Implemented scope

The branch restores versioned task priorities/runtime settings; registered-principal
policy and transactional recipients; durable ticket/lease/configuration/CLI retries;
independent shared workflows; project retirement with retained history/generations;
complete claim filtering; task access bindings and current label/lease evidence.
PostgreSQL remains the hosted authority. Existing task/lease/receipt owners are reused.

## Publication dependency

The implementation uses shared store receipt APIs absent from root main. Pin contracts,
connectors and both store dependency declarations to published root commit
`30ede402ca7d61b689f1a20ee364d69206f0ee98`. Validate this checkout outside the parent workspace,
so root path patches cannot hide dependency drift. Publish the existing 0eb461e history
first to make the root commit's submodule reachable. The exact pinned root commit must
remain reachable when root integration merges. No force push or source deletion.

## Acceptance and delivery

- Full library tests with required isolated PostgreSQL; no unavailable-store skips.
- Standalone Git dependency resolution/build, formatting and current dependency audit.
- Parent applicable build/tests/process probes and checked recursive checkout.
- PR CI on the actual published candidate, then authorized merge and main verification.
- Parent adopts the actual merged gitlink and validates that integration.

Validation receipt, 2026-11-22:

- Standalone `cargo test -- --test-threads=4`: 163 passed, zero failed/ignored,
  with PostgreSQL required and actual Git dependencies (no parent path patches).
- `cargo fmt --all -- --check`: passed.
- `cargo deny` with the parent's policy: advisories, bans and licenses passed.
  The parent's empty Git allowlist is specific to its path-patched workspace and refuses
  this standalone graph. `cargo metadata --locked` independently verified that its only
  Git source is the exact Devagotchi revision above; no registry policy was changed.
- Published `0eb461e` was verified on GitHub; a fresh recursive parent clone succeeded.
- Parent workspace validation found three hub integration failures; corrections and a
  complete rerun are underway. This library's standalone 163 tests passed independently.

Implementation: the library scope above is authored. Push: `0eb461e` published;
exact dependency pin prepared. PR/main: pending. Main merge does not close the parent's
remaining file/confinement/provider/guide/GUI/browser source obligations.

DELIVERY INCOMPLETE.

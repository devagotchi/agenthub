# Oldest-source absorption: board behavior

Source: preserved Go AgentHub `46419c697964016eb547c60416a8f2f5e854a95f`, first of the
five preserved sources. Local branch: `increment-288/absorb-october-baseline`. Parent
integration is devagotchi/devagotchi's plan `.ai/dev/plans/absorb-preserved-sources.md`
and its immutable `.ai/dev/plans/absorption/` file/commit ledger.

The operator requires complete oldest-first source authoring before new CI/pushes.
These are local commits within S1, not a separate helper/profile PR. Original source
checkouts remain untouched. Hosted data remains PostgreSQL; no SQLite fallback.

Current authored corrections from source store/edit.go, notes.go, hint.go and ops.go:

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

Local validation so far: nine focused checks passed before the lifecycle additions.
The subsequent library run passed 123 tests; refusal-help coverage found the new
not_completable code needed an explicit CLI next step. That mapping and its MCP counterpart
are now authored, and the affected refusal-help test passed. All 57 parent AgentHub-surface
tests then passed with PostgreSQL required. No full-workspace or CI claim is made for
this unfinished source batch.

Acceptance before source closure: every original file/commit mapped; domain and all
relevant surfaces implemented; source-specific checks, complete integration checks and
real platform/browser/Docker acceptance; green reviewed-head and main CI after all five
sources are authored; verified merge and only then branch deletion.

Implementation: partial. Validation: focused/local only. Push: not performed.
PR: not_created. Main: not delivered. **DELIVERY INCOMPLETE.**

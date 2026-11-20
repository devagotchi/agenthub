# Next October source feature: board audit history

Source a703761 store/events.go and dcca3d3 web/src/pages/Audit.tsx offer actor/action-prefix
filters, newest-first event history, event detail and chain verification, with live updates.
Rust already owns board events and verification but lacks a browser audit page and filtered,
newest-first queries. Extend that existing owner; preserve all board authorization.

Backend increment-280 will add a bounded EventFilter query with actor, action prefix,
ticket, exclusive after/before sequence cursors and descending order. Existing events()
semantics and full chain verification must remain unchanged. Filter in PostgreSQL before
limiting; escape literal wildcard characters. Tests must include filtering beyond the first
page, order/cursors, board/tenant isolation, no mutations, and chain verification after paging.

Parent increment-280 will add Audit on each board and a browser page in the existing app
frame, with localized labels, filters, event data escaped, task links, older-page cursor,
chain verification status and a refresh control. Live notification must use existing board
changes and recheck session/board access before returning any new result. Surface failures
without fabricated empty history. Shared-board viewers may read but gain no mutation access.
MCP board_events should expose the same filters, keeping its existing defaults.

Acceptance includes HTTP owner/member/viewer/foreign-account checks, actual PostgreSQL
permission failure, forged HTML escaped, invalid cursors refused, chain tampering visibly
reported, new events visible on live refresh, and Edge desktop/phone workflow with no overflow.

This restores board audit behavior only. The original global admin/principal/machine event
view also needs an owner-authorized hub call-audit interface through ops::audit; keep that gap
explicit until delivered. No historical source branch is complete from this one feature.

Backend implementation: `EventFilter` and `events_matching` in the existing HostedBoard,
with the synchronous Board forwarding method. Existing ascending/unbounded read and
chain verification retain their behavior. The read code moved out of hosted/mod.rs into
hosted/events.rs. No schema, writes or storage-owner changes.
Validation: all 96 AgentHub library tests passed against required isolated PostgreSQL,
zero ignored. Added tests read 206 real events, find a matching actor beyond the default
page, verify descending/exclusive cursor behavior and literal prefixes, prove board/tenant
isolation and ensure full chain verification still covers all 206 entries without mutation.
Formatting/diff checks passed. Backend push/PR/main and parent interface delivery pending.

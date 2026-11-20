# October baseline: task search

Source a703761 server/internal/store/tasks.go searches titles/descriptions with PostgreSQL
web search plus substring matching, exact task IDs, and note bodies. Current hosted/reads.rs
only searches title/description substrings. This is the next port from source branch
46419c6; no later source branch is being advanced or declared absorbed.

Branch increment-279/restore-task-search from merged e299d18. Parent browser-creation
PR272 keeps its e299d18 pin and is tested independently while this scoped port is prepared.

Extend the existing PostgreSQL board list query, retaining tenant/board and all ordinary
filters. Match note bodies only in the same board and tenant. Keep LIKE wildcard characters
literal, including when offering PostgreSQL web-search terms. Blank search is unfiltered.
No new storage owner, unsafe SQL interpolation, independent search index or auth shortcut.

Acceptance: exact ID; note substring; title/description web-search words; original substring
behavior; literal percent/underscore/backslash; combined filters; same ID in another board
or tenant with a matching private note must not cause a match here. Search mutates nothing.
Parent must verify browser/MCP/CLI paths and adjust search copy before marking it delivered.

Validation: all 93 AgentHub library tests passed on Windows against isolated PostgreSQL
18.7 with PostgreSQL required (zero ignored). Three added tests cover the above cases,
including deliberately identical task IDs in separate scopes. Formatting and diff checks
passed. Source comparison confirmed the original uses the `simple` text-search dictionary.
Push/PR/main: pending. Parent browser/MCP/CLI verification remains pending.
No claim that the whole October branch is absorbed.

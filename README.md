# AgentHub

The agents' board: tickets that any AI agent claims under a lease, works, annotates and hands
off, with approvals that wait for a person, notes that reach the agent they are for, and a
hash-chained history of every change.

Every board is in Postgres, either:

- **standalone**: a project's own board, worked with the `board` command line (`devagotchi board
  …`) and its hook pack for AI clients. `devagotchi board init` makes it in the Postgres
  `DEVAGOTCHI_BOARD_STORAGE_URL` names (or `--storage URL`, kept in `.devagotchi/config.json`
  and refused if it carries a password: `PGPASSWORD` or `~/.pgpass` holds that), under one fixed
  local tenant, the project's name its id; or
- **hosted**: one board per project of an account or organisation on a hub, its tenant held
  apart by row-level security, its events exported to ClickHouse when there is one.

Both run the same code (`src/hosted*`); a standalone board is the hosted board under the local
tenant. There was a SQLite standalone board before; nothing moves from a `board.sqlite3` (this is
pre-release, and there is no importer): `board init` makes a new, empty board.

AgentHub is part of [Devagotchi](https://github.com/devagotchi/devagotchi), which serves its
boards through its hub's MCP tools and pages and pulls this repository in as a submodule at
`crates/agenthub`. It builds and tests on its own as well.

## Building and testing

```sh
cargo test
```

The board's tests need a Postgres: point `DEVAGOTCHI_TEST_POSTGRES_URL` at a server (each test
makes a database of its own there). Without it they are skipped.

```sh
docker run -d --name agenthub-pg -e POSTGRES_HOST_AUTH_METHOD=trust -p 127.0.0.1:55432:5432 postgres:18-bookworm
DEVAGOTCHI_TEST_POSTGRES_URL=postgres://postgres@127.0.0.1:55432/postgres cargo test
```

## Postgres

AgentHub keeps its tables in schema `devagotchi`, all named `board_*`, with its own migration
history (`_agenthub_migrations`), so it shares a database with its host without either's
versions meeting the other's. A host runs its own migrations first, then AgentHub's:
`Store::open(url, &[&host, &devagotchi_agenthub::migrations::MIGRATIONS])`. A board's tenant is
the scope the host sets (`devagotchi.tenant`); AgentHub keeps no table of tenants.

## Layout

`src/hosted*` is the board in Postgres; `src/board/` and the protocol modules (`lease.rs`,
`approval.rs`, `inbox.rs` …) its synchronous face; `src/project.rs` the standalone board a project
opens; `src/cli/` the command line; `migrations/` the Postgres tables; `src/tests/` the
protocol's tests, by topic.
The rules are Devagotchi's (`.ai/patterns/layout.md` there).

## License

[Business Source License 1.1](LICENSE), as Devagotchi.

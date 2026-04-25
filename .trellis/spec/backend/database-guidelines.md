# Database / Persistence Guidelines

> Storage conventions for `reader-rs`.

> **Status**: **Not applicable yet.** The crate has no persistent storage. This document records the rules to follow **when** persistence is introduced, so the choice isn't made ad-hoc.

---

## When Persistence Lands

If/when `reader-rs` needs to remember state between runs (cached feeds, read/unread markers, OPML imports), the choice should be made deliberately and recorded here. The default expectation:

- **Embedded, single-user, file-backed.** A reader CLI does not need a server-class database.
- **Default candidate: SQLite via [`rusqlite`](https://docs.rs/rusqlite) or [`sqlx`](https://docs.rs/sqlx) with the `sqlite` feature.** Both ship a single-file DB; pick `sqlx` if compile-time-checked queries matter, `rusqlite` if dependency footprint matters more.
- **Plain-file fallback:** for very small state (config, cursor position), JSON/TOML in the user's data dir (via [`directories`](https://docs.rs/directories)) is fine. Don't reach for a DB until you have a relational query.

When that decision is made, replace this section with the chosen library, version, and rationale.

---

## Conventions to Adopt (when persistence exists)

These are the conventions to enforce **before** the first migration is merged. They prevent the schema from drifting into something that can't be reasoned about.

### Migrations

- **All schema changes go through migrations.** No manual `ALTER` against the live DB.
- **Migrations are append-only and idempotent.** Never edit a migration that has shipped; write a new one.
- **Numbered + named:** `migrations/0001_initial.sql`, `0002_add_read_status.sql`. Numbers monotonic, names slug-cased.
- **Up + down where reasonable.** Some changes (data loss) cannot be reversed; document that in the migration.
- **Tested on the previous-release schema** before merge — `cargo test` should run them against a temp DB.
- Use the migration runner that ships with the chosen library (e.g. `sqlx::migrate!`); don't roll your own.

### Naming

- `snake_case` for tables, columns, indexes. Tables singular (`feed_item`, not `feed_items`) — Rust types map 1:1.
- Primary key column: `id`. Foreign key column: `<referent>_id`.
- Boolean columns: `is_<predicate>` (`is_read`, `is_archived`).
- Timestamp columns: `created_at`, `updated_at`, `<event>_at`. Store as UTC.
- Index name: `idx_<table>_<columns>`; uniqueness: `uq_<table>_<columns>`.

### Query Patterns

- **Parameterise every query.** Never format user input into SQL. Both `rusqlite` and `sqlx` make this the default — don't fight it.
- **Use prepared statements / `query!` macros** so SQL is checked at compile time where possible.
- **Wrap multi-statement work in a transaction.** Failure mid-sequence must not leave half-applied state.
- **Don't `SELECT *`.** Name the columns; otherwise adding a column changes the row shape and breaks deserialisation silently.
- **Pagination uses keyset (`WHERE id > $cursor LIMIT n`)**, not `OFFSET`, for any list that grows.

### Connection Management

- **One connection (or one pool) per process**, owned by an application-level handle. Don't open ad-hoc connections inside helpers.
- **`Mutex<Connection>` is fine for SQLite single-writer**; don't reach for a real pool until measurements say so.
- **Close connections on shutdown** so SQLite's WAL is checkpointed.

### Errors

Map storage errors into the crate's typed `Error` enum (see `error-handling.md`); don't leak `rusqlite::Error` / `sqlx::Error` through the public API. Carry the offending key/path in the variant.

---

## Common Mistakes (to watch for once persistence ships)

- _Editing a shipped migration_ — every existing user's DB is now in a state your code doesn't expect.
- _Storing local time_ — TZ mismatches between machines silently corrupt ordering. Always UTC at rest.
- _String-concatenated queries_ — SQL injection or, in single-user CLI land, simply accidental quoting bugs.
- _`unwrap()` on a query result_ — every query is a fallible IO call. Propagate.
- _Schemas drifting from migrations_ — `cargo test` should run all migrations from scratch and assert the resulting schema.

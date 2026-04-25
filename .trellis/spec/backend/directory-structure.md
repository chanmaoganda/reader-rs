# Directory Structure

> How `reader-rs` source code is organized.

> **Status**: Bootstrap baseline. The crate currently contains only `src/main.rs`.
> Update this document with concrete file references as modules land.

---

## Crate Layout

`reader-rs` is a Rust 2024 binary crate (`Cargo.toml` → `[package] edition = "2024"`).
It will likely grow into a binary + library split so logic can be unit-tested without the binary entrypoint.

```
reader-rs/
├── Cargo.toml             # Manifest. Workspace not used yet.
├── Cargo.lock             # Committed (binary crate).
├── src/
│   ├── main.rs            # Binary entrypoint. Thin: parse args → call lib.
│   ├── lib.rs             # Library root. All reusable logic lives here.
│   ├── cli.rs             # CLI definition (clap derive) when added.
│   ├── config.rs          # Config loading and validation.
│   ├── error.rs           # Crate-level error type (see error-handling.md).
│   └── <feature>/         # One directory per cohesive feature/domain.
│       ├── mod.rs         # Public surface of the feature.
│       └── *.rs           # Internal submodules.
├── tests/                 # Integration tests. Each *.rs is a separate binary.
├── examples/              # Runnable usage examples (`cargo run --example <name>`).
└── benches/               # Criterion benchmarks (when needed).
```

### Binary + Library Split

Once the project grows past a single file, split into `main.rs` + `lib.rs`:

- `main.rs` does **only** argument parsing, logger init, and dispatch to `reader_rs::run(...)`.
- All business logic, types, and tests live under `lib.rs` so they are reachable from integration tests in `tests/`.

`main.rs` should be small enough that it has nothing meaningful to unit-test.

---

## Module Organization

### Prefer files over directories

Use `src/foo.rs` until the module needs siblings; only then promote to `src/foo/mod.rs` (or `src/foo.rs` + `src/foo/`, both supported in edition 2024). Avoid creating a directory with a single `mod.rs`.

### One concept per module

A module groups types and functions that share invariants or a domain concept. If a module has multiple unrelated responsibilities, split it.

### Keep `pub` minimal

Default to private (`fn` / `struct`). Promote to `pub(crate)` for cross-module use, and only to `pub` for the library's published API. Re-export the public surface from `lib.rs`.

### Tests live next to code

Unit tests go in a `#[cfg(test)] mod tests { ... }` block at the bottom of the file they test. Integration tests live under `tests/`.

---

## Naming Conventions

Follow [RFC 430](https://rust-lang.github.io/rfcs/0430-finalizing-naming-conventions.html) — `cargo clippy` will flag deviations.

| Item | Convention | Example |
|------|------------|---------|
| Crate / module / file | `snake_case` | `feed_parser.rs` |
| Type / trait / enum | `UpperCamelCase` | `FeedItem`, `Reader` |
| Function / method / variable | `snake_case` | `parse_feed`, `item_count` |
| Constant / static | `SCREAMING_SNAKE_CASE` | `MAX_ITEMS` |
| Lifetime | short lowercase, single quote | `'a`, `'src` |
| Type parameter | single uppercase or descriptive `UpperCamelCase` | `T`, `Item` |

Avoid stuttering: `feed::Feed` rather than `feed::FeedFeed`; `error::Error` rather than `error::ReaderError`.

---

## Examples

Link real modules here as they are added:

- _(none yet — populate after the first feature lands)_

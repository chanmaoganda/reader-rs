# Backend Development Guidelines

> Conventions for `reader-rs` (Rust 2024 binary crate).

The `backend` layer name is inherited from Trellis's default scaffold; for this project it covers all Rust source code (`src/`). There is no separate frontend.

---

## Pre-Development Checklist

Before writing or modifying code, read the files relevant to your change:

- **Always**: [Quality Guidelines](./quality-guidelines.md) — lints, forbidden patterns, review checklist.
- **Adding/changing modules or files**: [Directory Structure](./directory-structure.md).
- **Touching error types or `Result`-returning functions**: [Error Handling](./error-handling.md).
- **Adding logs / observability**: [Logging Guidelines](./logging-guidelines.md).
- **Introducing storage** (none yet): [Database Guidelines](./database-guidelines.md).

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Directory Structure](./directory-structure.md) | Crate / module organization and file layout | Bootstrap baseline |
| [Error Handling](./error-handling.md) | `Result`, `thiserror` / `anyhow`, panics | Bootstrap baseline |
| [Logging Guidelines](./logging-guidelines.md) | `tracing`, levels, structured fields | Bootstrap baseline |
| [Quality Guidelines](./quality-guidelines.md) | Lints, forbidden patterns, review checklist | Bootstrap baseline |
| [Database Guidelines](./database-guidelines.md) | Persistence conventions (no storage yet) | Pending first use |

> **Note**: `reader-rs` is a single-binary Rust 2024 crate (`src/main.rs` only) as of 2026-04-25. Each guideline calls out aspirational vs. observed rules; add references to real modules as they land.

---

## How to Fill These Guidelines

For each guideline file:

1. Document your project's **actual conventions** (not ideals)
2. Include **code examples** from your codebase
3. List **forbidden patterns** and why
4. Add **common mistakes** your team has made

The goal is to help AI assistants and new team members understand how YOUR project works.

---

**Language**: All documentation should be written in **English**.

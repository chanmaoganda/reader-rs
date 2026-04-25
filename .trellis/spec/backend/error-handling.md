# Error Handling

> Error conventions for `reader-rs`.

> **Status**: Bootstrap baseline. Update with crate-specific error variants once they exist.

---

## Core Rules

1. **Fallible operations return `Result<T, E>`.** Never use a sentinel value (e.g. `-1`, empty string) to indicate failure.
2. **Propagate with `?`.** No manual `match` for the sole purpose of re-bubbling.
3. **No `unwrap()` / `expect()` / `panic!()` / `unreachable!()` outside tests** — except where a comment documents the invariant that makes the call infallible. When it is justified, prefer `expect("<invariant>")` over `unwrap()` so the panic message names the broken assumption.
4. **Errors carry context.** A `std::io::Error` saying "No such file" without the path is useless. Add the path, URL, or offending input at the boundary that knows it.
5. **Library code defines typed errors. Binary/glue code may use `anyhow`.** See below.

---

## Error Types

### Library layer — `thiserror`

Code under `lib.rs` should expose a domain-specific error enum derived with [`thiserror`](https://docs.rs/thiserror). Callers get to `match` on variants.

```rust
// src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to read feed from {path}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid feed format: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

Guidelines:

- Each variant describes **one** failure mode. Don't fold unrelated cases under a `Other(String)` catch-all.
- Use `#[source]` (or `#[from]`) to chain underlying errors — preserves the cause chain.
- Use `#[error("...")]` messages that read like a sentence and identify the offending value. No trailing period.
- Errors should be `Send + Sync + 'static` so they cross thread/async boundaries (this is automatic if every field is).

### Application layer — `anyhow`

`main.rs` and short-lived application glue may return `anyhow::Result<()>` and use `.context("…")` / `.with_context(|| …)` to attach human-readable context. Use `anyhow!("…")` for ad-hoc errors that don't deserve a typed variant.

```rust
fn main() -> anyhow::Result<()> {
    let cfg = config::load().context("loading config")?;
    reader_rs::run(cfg).context("running reader")?;
    Ok(())
}
```

Do **not** import `anyhow::Error` into the library API surface — keep `lib.rs` typed.

---

## Error Handling Patterns

### Adding context

```rust
let bytes = std::fs::read(&path)
    .map_err(|source| Error::Io { path: path.clone(), source })?;
```

Or, with `anyhow`:

```rust
let bytes = std::fs::read(&path)
    .with_context(|| format!("reading feed from {}", path.display()))?;
```

### Converting between error types

Prefer `#[from]` on a `thiserror` variant over hand-written `From` impls — it makes the bare `?` operator do the right thing.

### Don't swallow

```rust
// ❌ The error is gone forever.
let _ = risky();

// ✅ Either propagate, or log + continue with a documented reason.
if let Err(err) = risky() {
    tracing::warn!(?err, "risky() failed; continuing with default");
}
```

### `Option` vs `Result`

- `Option<T>` for absence that is **not** an error (lookup miss, optional field).
- `Result<T, E>` when the caller needs to know **why** something failed.
- Convert with `.ok_or(Error::…)` / `.ok_or_else(|| …)` at the boundary.

---

## Panics

A panic is an unrecoverable bug, not a control-flow tool. Acceptable panic sites:

- Test code (`#[test]`, `#[should_panic]`).
- `assert!` / `debug_assert!` for invariants the type system can't express.
- Documented "unreachable" branches (`unreachable!("...")`) where omitting them would force fallback handling for a state that cannot occur given the type's invariants.

Outside those, return an error.

---

## Common Mistakes

_(populate as real incidents occur)_

- _Stringly-typed errors_ — avoid `Err("something broke".into())`. Add a typed variant.
- _Lossy `From` chains_ — `From<io::Error>` that drops the path is worse than no `From`.
- _Logging then re-throwing_ — log **or** propagate, not both. Logging at every layer floods the output; the top-level handler logs the full chain once.

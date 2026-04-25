# Logging Guidelines

> How `reader-rs` emits diagnostic output.

> **Status**: Bootstrap baseline. The crate has no logging yet; these are the conventions to adopt.

---

## Library: `tracing`

Use the [`tracing`](https://docs.rs/tracing) ecosystem, not `log` and not `println!` / `eprintln!`.

- `tracing` for emitting events and spans inside library code.
- `tracing-subscriber` for formatting and filtering, configured **only** in `main.rs`.

```toml
# Cargo.toml
[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

### Initialize once, in `main`

```rust
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,reader_rs=debug".into()),
        )
        .init();
    // …
}
```

Library code must never call `*::init()`; it would clobber the binary's choice.

---

## Log Levels

| Level   | When to use                                                                 |
|---------|-----------------------------------------------------------------------------|
| `error` | Operation failed and the caller cannot recover. Often paired with bubbling an `Error` up. |
| `warn`  | Surprising state, recovered or skipped. Investigate if it recurs.            |
| `info`  | High-value lifecycle events: startup, shutdown, completing a unit of work.   |
| `debug` | Per-operation detail useful when diagnosing. Off by default in releases.     |
| `trace` | Fine-grained, high-volume. Off unless explicitly enabled.                    |

Rule of thumb: **a fresh `info`-level log line for every successful run should still be readable**. If `info` is noisy, demote.

---

## Structured Logging

Always emit fields, never preformat into the message string.

```rust
// ❌ String concatenation — fields are not extractable downstream.
tracing::info!("fetched {url} in {ms}ms", url = url, ms = elapsed.as_millis());

// ✅ Structured fields.
tracing::info!(url = %url, elapsed_ms = elapsed.as_millis(), "fetched feed");
```

Sigils:

- `%value` — uses `Display`. Use for IDs, URLs, paths.
- `?value` — uses `Debug`. Use for arbitrary structs and errors.
- bare `key = expr` — for primitives.

### Spans for scoped work

Wrap multi-step operations in a span so events inherit context:

```rust
let span = tracing::info_span!("fetch_feed", %url);
let _enter = span.enter();
// every event here automatically carries `url`
```

For async, prefer `.instrument(span)` over `enter()`.

---

## What to Log

- Process lifecycle: startup config (sanitised), shutdown, signal received.
- Boundaries: incoming requests / commands, outgoing network calls (URL, status, duration).
- Recovered errors: log once with `tracing::warn!(?err, …)`.
- Unrecoverable errors: logged once at the top level (typically in `main`) with the full chain.

---

## What NOT to Log

- **Secrets**: API keys, tokens, passwords, signed cookies, authorization headers. Redact at the source — wrap in a newtype with a `Debug` impl that prints `"<redacted>"` if you must store them.
- **Personal data** beyond what the operation requires.
- **Full payloads** by default. Log size + a hash or truncated prefix; gate the full body behind `trace`.
- **Per-iteration spam** in hot loops at `info`+. Demote to `debug` or aggregate.

---

## Don't mix `println!` with `tracing`

Application output for the user (the actual feed render, CLI results) goes to stdout via `println!` / `writeln!`. Diagnostics go to `tracing`, which writes to stderr by default. Keep the two streams separate so users can pipe stdout without losing logs.

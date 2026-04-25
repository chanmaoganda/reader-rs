# Quality Guidelines

> Code quality bar for `reader-rs`.

> **Status**: Bootstrap baseline. Tighten as patterns emerge; never loosen without recording the reason.

---

## Required Tooling (must pass before commit)

```bash
cargo fmt --all -- --check          # formatting
cargo clippy --all-targets -- -D warnings   # lints, warnings = errors
cargo test --all-targets            # unit + integration + doc tests
cargo build --release               # release builds must compile too
```

`cargo clippy -D warnings` is non-negotiable in CI. Locally, prefer fixing the warning over `#[allow(...)]`. If you must allow, scope it as narrowly as possible (`#[allow(clippy::lint_name)]` on the specific item) and add a comment with the reason.

---

## Required Patterns

- **`Result<T, E>` for fallible work.** See `error-handling.md`.
- **`tracing` for diagnostics.** See `logging-guidelines.md`. Never `println!` / `eprintln!` for logs.
- **Doc comments on every `pub` item.** `///` for items, `//!` for module/crate. Include at least one runnable `# Examples` block on `pub` API of the library.
- **`#[must_use]` on `Result`-returning functions whose errors callers might ignore**, and on builder-style methods. Clippy will catch most cases.
- **Newtype wrappers for domain values** (e.g. `FeedId(String)`, `ItemCount(u32)`) — gives type safety and a place to attach validation.
- **Borrow over own at API boundaries:** prefer `&str` to `String`, `&[T]` to `Vec<T>`, `impl AsRef<Path>` to `&Path` for file-taking functions, unless the function actually needs ownership.
- **`#[non_exhaustive]` on public enums and structs that may grow** — keeps adding a variant from being a breaking change.

---

## Forbidden Patterns

| Pattern | Why | Use instead |
|---------|-----|-------------|
| `unwrap()` / `expect()` outside tests, without a SAFETY/INVARIANT comment | Crashes production on unexpected input | `?` propagation, `let … else { return Err(...) }` |
| `panic!("…")` for control flow | Same | Return an error |
| `.clone()` to silence the borrow checker | Hides ownership confusion; perf cost | Rework lifetimes / borrow / `Cow` |
| `Vec<u8>` / `String` parameters when callee only reads | Forces unnecessary allocations on the caller | `&[u8]` / `&str` / `impl AsRef<…>` |
| `Box<dyn Error>` in library APIs | Erases types, blocks `match`-on-variant | `thiserror` enum (see `error-handling.md`) |
| `unsafe` without a `// SAFETY:` comment justifying every invariant | Reviewers can't verify soundness | Either prove safety in a comment or use a safe API |
| Catch-all `_` arms on small enums | Defeats exhaustiveness; hides new variants | Match each variant explicitly; reach for `_` only on truly open enums |
| Hand-written `From<io::Error>` that drops context (path, URL) | Makes errors useless | Carry the offending input in the error variant |
| `as` casts between numeric types | Silently truncates / wraps | `try_into()` (returns `Result`) or `i32::try_from(x)?` |
| Locking a `Mutex` across an `.await` | Deadlock risk under task cancellation | `tokio::sync::Mutex`, or release before awaiting |

---

## `unsafe`

`unsafe` requires:

1. A `// SAFETY:` comment on every `unsafe` block enumerating the invariants the caller relies on and why they hold here.
2. The smallest possible block — wrap exactly the operation that needs it, no surrounding "convenient" lines.
3. A safe wrapper at the module boundary so callers don't see `unsafe`.
4. A reviewer who reads the SAFETY comment and disagrees blocks the PR until resolved.

If you're reaching for `unsafe` for performance, benchmark the safe version first.

---

## Testing Requirements

- **Unit tests** in a `#[cfg(test)] mod tests` block at the bottom of the file under test. Pure logic should be tested at this level.
- **Integration tests** in `tests/` for behaviour that crosses module boundaries or hits the public API surface.
- **Doc tests** on public API examples — they run under `cargo test` and prevent docs from rotting.
- **Property tests** (`proptest`) encouraged for parsers and anything with combinatorial inputs.
- A new feature lands with tests that fail before the change and pass after it. A bug fix lands with a regression test.
- Tests must be deterministic — no real network, no real clock, no real sleep. Inject the dependency or use fakes.

Coverage is not enforced as a number; the rule is "every behaviour the public API promises has at least one test".

---

## Comments and Documentation

- Comments answer **why**, not what. The code already says what.
- Don't write comments that restate the function name (`// parses the feed` above `fn parse_feed`).
- Don't reference tasks / PRs / authors in comments — that information rots and lives better in `git blame` and the PR description.
- A `TODO`/`FIXME` comment must include a tracking issue or a concrete trigger ("remove once X stabilises"); naked TODOs accumulate.

---

## Code Review Checklist

- [ ] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` all green locally
- [ ] No new `unwrap()` / `expect()` / `panic!()` outside tests (or each is justified by a comment)
- [ ] No new `unsafe` without a `// SAFETY:` block per invariant
- [ ] Public items have doc comments; new public API has an example
- [ ] Errors carry the offending input (path, URL, identifier)
- [ ] Logs are structured; no `println!` for diagnostics; no secrets in fields
- [ ] Tests added for new behaviour; regression test added for bug fixes
- [ ] No dead code, no commented-out code, no scratch debug `dbg!()`

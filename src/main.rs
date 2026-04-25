//! `reader-rs` binary entrypoint.
//!
//! Initialises diagnostics and hands off to the library `run` function.
//! All business logic lives in [`reader_rs`]; this file stays thin so that
//! integration tests and benches can exercise the same code paths.

use anyhow::Context;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,reader_rs=debug")),
        )
        .init();

    reader_rs::run().context("running reader-rs")?;
    Ok(())
}

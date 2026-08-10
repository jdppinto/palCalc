//! `palcalc-fetch <url> <sha256-fingerprint>` — fetch a roster from a pinned
//! palcalc-server and print it to stdout.
//!
//! The bearer token is read from the `PALCALC_TOKEN` environment variable
//! (not an argument) so it doesn't leak into the process list / shell history.
//!
//! This is the reference implementation of the pinning transport that the
//! palCalc app's "server mode" will port into its Tauri backend.
//!
//! Example:
//!   PALCALC_TOKEN=... palcalc-fetch https://your-host:8122/roster AA:BB:...:FF

use std::io::Write;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: PALCALC_TOKEN=<token> palcalc-fetch <url> <sha256-fingerprint>"))?;
    let fingerprint = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing <sha256-fingerprint> argument"))?;
    let token = std::env::var("PALCALC_TOKEN")
        .map_err(|_| anyhow::anyhow!("set the PALCALC_TOKEN environment variable"))?;

    let body = palcalc_server::pin::fetch(&url, &token, &fingerprint).await?;
    std::io::stdout().write_all(&body)?;
    Ok(())
}

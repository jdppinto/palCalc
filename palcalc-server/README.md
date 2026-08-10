# palcalc-server

A read-only **HTTPS** server that reads a Palworld dedicated-server save (via
[`palm-save`]) and serves the parsed roster — every pal with its species,
gender, level, passives, owner, and location (**palbox / party / base**) — to
authenticated palCalc clients. No OCR, no scanning: the data comes straight
from the save.

It is designed to face an open, port-forwarded internet port. See the security
posture below and [`docs/CLIENT-PINNING.md`](docs/CLIENT-PINNING.md).

## Setup

```
cp palcalc-server.example.toml palcalc-server.toml   # then edit it
cargo run --release                                  # reads ./palcalc-server.toml
```

`palcalc-server.toml` holds the save directory, bind address, and one bearer
**token per friend** (generate long random values). It is git-ignored — never
commit it. On first run a self-signed TLS cert/key are generated and the
certificate's **SHA-256 fingerprint** is printed; share that (and each token)
with clients out-of-band so they can pin it.

> **Port:** use one **distinct from your Palworld game port**. The game is UDP
> and this server is TCP, so they can share a number at the OS level, but your
> router's game-port forward is UDP-only — forward **TCP** for this port
> separately. The default is `8123`; connect clients to
> `https://<host>:8123/roster`.

Endpoints:

- `GET /health` — open liveness check (`{"status":"ok"}`).
- `GET /roster` — bearer-authenticated; returns the whole roster as JSON.

## Security posture

- **TLS only** (rustls / ring). Self-signed cert auto-generated; fingerprint
  printed for client pinning. Private key written `0600`, created atomically.
- **Bearer auth**, constant-time compared (SHA-256 digests), on every data
  request. Configured tokens must be ≥24 chars.
- **No request input touches the filesystem** — two fixed routes, save dir is
  operator-config only.
- **Bounded**: per-IP + global rate limits, request timeout, 1 KB body cap,
  global in-flight cap, and a stampede-proof mtime cache (the ~70 MB save is
  never re-parsed per request).
- Generic error responses; details logged server-side only.

Connection/handshake-volume floods are a network-layer concern — front the port
with a firewall/fail2ban. Keep the save directory operator-only.

## Client (feature `client`)

The `client` feature builds the pinning HTTPS client and the `palcalc-fetch`
reference CLI:

```
cargo run --features client --bin palcalc-fetch -- https://host:8123/roster <fingerprint>
# token from the PALCALC_TOKEN env var
```

The pinning verifier (`src/pin.rs`) accepts the server's self-signed cert only
if its fingerprint matches AND the handshake signature verifies — real MITM
protection. This is the reference the palCalc app's "server mode" will port
into its Tauri backend. See [`docs/CLIENT-PINNING.md`](docs/CLIENT-PINNING.md).

[`palm-save`]: https://github.com/jdppinto/palm-save

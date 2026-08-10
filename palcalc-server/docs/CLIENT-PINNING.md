# Client certificate pinning

palcalc-server serves TLS with a **self-signed** certificate, and it is reached
at whatever host/IP the operator port-forwards — a name that won't match the
cert's SANs. So ordinary CA + hostname validation can't authenticate it. The
client instead **pins** the server's certificate by its SHA-256 fingerprint.

This is what makes the open port MITM-safe. Without pinning, TLS gives you
encryption but *no proof of who you're talking to*: an on-path attacker can
present their own certificate, and a client that "accepts any cert" would hand
over the bearer token and roster to the attacker.

## Trust model (both conditions are required)

A pinned client accepts the connection only if **both** hold:

1. **Fingerprint match** — the presented leaf certificate's SHA-256 equals the
   pinned value the operator shared out-of-band. (We trust exactly one
   certificate, not "any self-signed cert".)
2. **Handshake signature is still verified** against that certificate. The
   certificate is public (an eavesdropper can copy it), so pinning alone is not
   enough — the client must still confirm the peer holds the matching private
   key. `pin.rs` delegates signature verification to the real crypto provider
   for exactly this reason. **Never** stub this out; doing so makes pinning
   worthless.

Hostname and CA validation are intentionally bypassed and *replaced* by (1)+(2).

## Getting the fingerprint

The server prints it at startup:

```
INFO palcalc_server::tls: TLS certificate SHA-256 fingerprint: 40:B8:05:...:A3
INFO palcalc_server::tls: clients should pin this fingerprint (self-signed cert)
```

The operator sends this string (and each friend's token) over a channel the
friend trusts — Discord DM, Signal, in person. The fingerprint is **not**
secret, but it must arrive un-tampered: if an attacker can alter the fingerprint
in transit, they can substitute their own.

The fingerprint changes whenever the certificate is regenerated (e.g. `cert.pem`
/ `key.pem` are deleted). Re-share it after any regeneration.

## Using it

Reference implementation: `src/pin.rs` (the `pin::fetch` function and the
`PinnedVerifier`) and the `palcalc-fetch` CLI:

```
# token is read from the environment, never an argument (process-list hygiene)
PALCALC_TOKEN=<your-token> \
  palcalc-fetch https://your-host:8122/roster 40:B8:05:...:A3
```

- Correct fingerprint + correct token → roster JSON on stdout.
- Wrong token → `HTTP 401`.
- Wrong/tampered fingerprint → the TLS handshake is refused.

## Integrating into the palCalc app ("server mode")

Do the request in the **Rust (Tauri) backend**, not the webview:

- The webview's `fetch` uses the OS/browser trust store and can't pin a
  self-signed cert to an arbitrary host — it would either reject the cert or
  force the user to disable verification (unsafe).
- Port `pin.rs` into the Tauri backend and expose a command like
  `fetch_server_roster(url, token, fingerprint)` that returns the JSON. The
  frontend stores the URL + fingerprint (public) and the token (treat as a
  secret) and calls that command.

Server response shape (`GET /roster`): `{ generated_at_unix, players:[{uid,
name}], pals:[{species, gender, level, passives, owner, location, container}] }`
where `location` ∈ `palbox | party | base | unknown`. Mapping the Palworld
internal `species`/`passives` names to palCalc's tribe/passive keys happens
client-side (the app already has that data).

## Operational notes

- **Token** is the credential — keep it secret, one per friend, revoke by
  removing the line from `palcalc-server.toml` and restarting.
- **Connection/handshake-volume floods** are a network-layer concern; front the
  port with a firewall / fail2ban. The app's per-IP + global rate limits bound
  request work, not raw TCP/TLS accept cost.
- The save directory must remain **operator-only** — the parser assumes the
  save is trusted (see the palm-save notes).

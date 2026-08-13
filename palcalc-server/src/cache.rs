//! Roster cache: parse the (large) save at most once per change.
//!
//! Parsing `Level.sav` is CPU- and memory-heavy (tens of MB decompressed), so
//! we never parse per request. The serialized JSON response is cached and
//! reused until the save's mtime changes. A single async mutex serializes
//! reparses, so a burst of requests triggers exactly one parse (no stampede),
//! and the expensive work runs on a blocking thread so the async runtime is
//! never stalled.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bytes::Bytes;
use palm_save::{classify, parse_dps, parse_player, parse_roster, Ivs, Location, PlayerLoc};
use serde::Serialize;
use tokio::sync::Mutex;

pub struct RosterCache {
    dir: PathBuf,
    inner: Mutex<Option<Cached>>,
}

struct Cached {
    /// Serialized JSON body. `Bytes` clones are O(1) (refcounted), so every
    /// request shares one allocation.
    body: Bytes,
    /// mtime signature the body was built from.
    sig: Sig,
}

/// A cheap change signal: the mtimes of `Level.sav` and the `Players/` dir.
#[derive(PartialEq, Clone, Copy)]
struct Sig {
    level: Option<SystemTime>,
    players: Option<SystemTime>,
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl RosterCache {
    pub fn new(dir: PathBuf) -> RosterCache {
        RosterCache {
            dir,
            inner: Mutex::new(None),
        }
    }

    fn current_sig(&self) -> Sig {
        Sig {
            level: mtime(&self.dir.join("Level.sav")),
            players: mtime(&self.dir.join("Players")),
        }
    }

    /// Return the current roster JSON body, reparsing only if the save changed.
    /// Errors are returned to the caller (mapped to a generic 500 upstream);
    /// details are never surfaced to clients.
    pub async fn body(&self) -> anyhow::Result<Bytes> {
        let sig = self.current_sig();
        let mut guard = self.inner.lock().await;
        if let Some(c) = guard.as_ref() {
            if c.sig == sig {
                return Ok(c.body.clone());
            }
        }
        // Rebuild. Hold the lock across the parse so concurrent callers wait
        // for this single parse rather than each starting their own.
        //
        // Accepted tradeoff: if this caller's request future is cancelled
        // (e.g. the request timeout fires) the guard drops and the detached
        // `spawn_blocking` parse keeps running; a waiting caller could then
        // start a second parse. This is bounded and low-risk in practice —
        // a full parse takes ~1-2s while `request_timeout_secs` defaults to
        // 30s, so cancellation mid-parse effectively never happens. Keep the
        // timeout comfortably above worst-case parse time. (A full fix would
        // detach the parse into a task whose result survives caller
        // cancellation; not worth the complexity at this scale.)
        let dir = self.dir.clone();
        let body: Bytes = tokio::task::spawn_blocking(move || build_body(&dir))
            .await
            .map_err(|e| anyhow::anyhow!("roster parse task failed: {e}"))??
            .into();
        *guard = Some(Cached {
            body: body.clone(),
            sig,
        });
        Ok(body)
    }
}

fn read_gvas(path: &Path) -> anyhow::Result<Vec<u8>> {
    let raw = std::fs::read(path)?;
    if raw.starts_with(b"GVAS") {
        Ok(raw)
    } else {
        Ok(palm_save::decompress_sav(&raw)?)
    }
}

fn hx(u: &[u8; 16]) -> String {
    u.iter().map(|b| format!("{b:02x}")).collect()
}

/// Player save / DPS filenames use the canonical UUID hex (first 4 bytes shown
/// big-endian); the roster's owner ids are the raw little-endian bytes. Reverse
/// the first 4 bytes (2 hex chars each), lowercased, so a DPS file's owner
/// matches the roster's player ids.
fn owner_hex_from_filename_uid(uid: &str) -> Option<String> {
    if uid.len() != 32 || !uid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let u = uid.to_lowercase();
    Some(format!("{}{}{}{}{}", &u[6..8], &u[4..6], &u[2..4], &u[0..2], &u[8..]))
}

#[derive(Serialize)]
struct PlayerOut {
    uid: String,
    name: String,
    /// The guild (group id) this player belongs to.
    guild: Option<String>,
}

/// Innate stat talents (IVs), each 0..=100. Serialized only when present.
#[derive(Serialize, Clone, Copy)]
struct IvsOut {
    hp: i64,
    attack: i64,
    defense: i64,
}

impl From<Ivs> for IvsOut {
    fn from(v: Ivs) -> Self {
        IvsOut { hp: v.hp, attack: v.attack, defense: v.defense }
    }
}

#[derive(Serialize)]
struct PalOut {
    species: String,
    gender: String,
    level: i64,
    passives: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ivs: Option<IvsOut>,
    owner: Option<String>,
    location: &'static str,
    container: Option<String>,
    /// The guild (group id) that owns this pal's container/base — authoritative
    /// for the "my guild's bases" view (not inferred from the pal's owner).
    guild: Option<String>,
}

#[derive(Serialize)]
struct RosterOut {
    /// Server-side generation time (unix seconds), for client display/caching.
    generated_at_unix: u64,
    players: Vec<PlayerOut>,
    pals: Vec<PalOut>,
}

/// Parse the save dir and serialize the roster. Runs on a blocking thread.
fn build_body(dir: &Path) -> anyhow::Result<Vec<u8>> {
    let level = read_gvas(&dir.join("Level.sav"))?;
    let roster = parse_roster(&level).map_err(|e| anyhow::anyhow!("parse_roster: {e}"))?;
    // Free the ~72MB Level.sav buffer before the DPS files (each ~73MB) are
    // read one at a time below, keeping peak memory to a single save.
    drop(level);

    // Load each real player save for palbox/party labeling.
    let mut players_loc: Vec<PlayerLoc> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir.join("Players")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !name.ends_with(".sav") || name.ends_with("_dps.sav") {
                continue;
            }
            match read_gvas(&path).and_then(|g| {
                parse_player(&g).map_err(|e| anyhow::anyhow!("parse_player {name}: {e}"))
            }) {
                Ok(loc) => players_loc.push(loc),
                // A single bad player save must not sink the whole roster;
                // its pals simply fall through to "base"/"unknown" labeling.
                Err(e) => tracing::warn!("skipping player save {name}: {e}"),
            }
        }
    }

    let players = roster
        .players
        .iter()
        .map(|(uid, name)| PlayerOut {
            uid: hx(uid),
            name: name.clone(),
            guild: roster.player_guild.get(uid).map(hx),
        })
        .collect();

    let mut pals: Vec<PalOut> = roster
        .pals
        .iter()
        .map(|p| PalOut {
            species: p.species.clone(),
            gender: p.gender.clone(),
            level: p.level,
            passives: p.passives.clone(),
            ivs: p.ivs.map(Into::into),
            owner: p.owner.map(|o| hx(&o)),
            location: match classify(p.container, &players_loc) {
                Location::Palbox => "palbox",
                Location::Party => "party",
                Location::Base => "base",
                Location::Unknown => "unknown",
            },
            container: p.container.map(|c| hx(&c)),
            guild: p.guild.map(|g| hx(&g)),
        })
        .collect();

    // Append each player's Dimensional Pal Storage pals (from <uid>_dps.sav),
    // which live outside Level.sav. Read one file at a time (each ~73MB
    // decompressed); owner comes from the filename, guild from that player.
    let guild_by_owner: std::collections::HashMap<String, String> = roster
        .player_guild
        .iter()
        .map(|(u, g)| (hx(u), hx(g)))
        .collect();
    if let Ok(entries) = std::fs::read_dir(dir.join("Players")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if !name.ends_with("_dps.sav") {
                continue;
            }
            let canon = &name[..name.len() - "_dps.sav".len()];
            let owner = owner_hex_from_filename_uid(canon);
            let guild = owner.as_ref().and_then(|o| guild_by_owner.get(o).cloned());
            match read_gvas(&path)
                .and_then(|g| parse_dps(&g).map_err(|e| anyhow::anyhow!("parse_dps {name}: {e}")))
            {
                Ok(dps) => {
                    for p in &dps {
                        pals.push(PalOut {
                            species: p.species.clone(),
                            gender: p.gender.clone(),
                            level: p.level,
                            passives: p.passives.clone(),
                            ivs: p.ivs.map(Into::into),
                            owner: owner.clone(),
                            location: "dps",
                            container: None,
                            guild: guild.clone(),
                        });
                    }
                }
                Err(e) => tracing::warn!("skipping DPS {name}: {e}"),
            }
        }
    }

    let out = RosterOut {
        generated_at_unix: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        players,
        pals,
    };
    Ok(serde_json::to_vec(&out)?)
}

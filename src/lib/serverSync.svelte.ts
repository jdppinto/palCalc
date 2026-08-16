// Background auto-sync: when the user has enabled it and a connection was made
// before (saved URL + fingerprint + token in the shared config), re-fetch the
// palcalc-server roster every 20s and refresh the server-sourced owned pals.
// This lives outside ServerImport.svelte so it keeps running regardless of which
// tab is open or whether the "From server" panel is mounted.
import { invoke } from "@tauri-apps/api/core";
import type {
  OwnedPal,
  PalEntry,
  PassiveEntry,
  ServerPal,
  ServerRoster,
} from "./types";
import { replaceServerOwned } from "./owned.svelte";

const LS_KEY = "palcalc.server";
const POLL_MS = 20_000;
const PREFIXES = ["BOSS_", "GYM_", "RAID_", "PREDATOR_"];

export interface ServerConf {
  url: string;
  fingerprint: string;
  token: string;
  owner: string; // "" = everyone
  autoSync: boolean;
}

/// Read the shared connection config that ServerImport persists.
export function readServerConf(): ServerConf {
  try {
    const s = JSON.parse(localStorage.getItem(LS_KEY) || "{}");
    return {
      url: (s.url || "").trim(),
      fingerprint: (s.fingerprint || "").trim(),
      token: (s.token || "").trim(),
      owner: s.owner || "",
      autoSync: !!s.autoSync,
    };
  } catch {
    return { url: "", fingerprint: "", token: "", owner: "", autoSync: false };
  }
}

export function hasSavedConnection(c: ServerConf = readServerConf()): boolean {
  return !!c.url && !!c.fingerprint && !!c.token;
}

// Catalogs for mapping the save's internal keys → palCalc's. Loaded once and
// shared with ServerImport so the two agree on how a ServerPal becomes an
// OwnedPal.
let validSpecies = new Set<string>();
let validPassives = new Set<string>();
let displayName = new Map<string, string>();
let catalogsLoaded = false;

export async function ensureCatalogs(): Promise<void> {
  if (catalogsLoaded) return;
  const pals = await invoke<PalEntry[]>("list_pals");
  validSpecies = new Set(pals.map((p) => p.key));
  displayName = new Map(pals.map((p) => [p.key, p.name]));
  const pass = await invoke<PassiveEntry[]>("list_passives");
  validPassives = new Set(pass.map((p) => p.key));
  catalogsLoaded = true;
}

function normSpecies(s: string): string | null {
  if (validSpecies.has(s)) return s;
  for (const p of PREFIXES) {
    if (s.startsWith(p)) {
      const t = s.slice(p.length);
      if (validSpecies.has(t)) return t;
    }
  }
  return null;
}

/// Map a server pal to an OwnedPal, or null if its species isn't recognized.
export function serverPalToOwned(p: ServerPal): OwnedPal | null {
  const species = normSpecies(p.species);
  if (!species) return null;
  return {
    species,
    label: displayName.get(species) || species,
    passives: p.passives.filter((k) => validPassives.has(k)),
    gender: p.gender === "Male" ? "Male" : p.gender === "Female" ? "Female" : null,
    level: p.level,
    location: p.location,
    guild: p.guild,
    source: "server",
  };
}

/// A player's own palbox/party/dimensional pals plus their whole guild's base
/// pals; "" selects the entire roster. Mirrors ServerImport's selection.
export function filterForOwner(roster: ServerRoster, owner: string): ServerPal[] {
  if (!owner) return roster.pals;
  const guild = roster.players.find((p) => p.uid === owner)?.guild ?? null;
  return roster.pals.filter(
    (p) =>
      (p.owner === owner &&
        (p.location === "palbox" || p.location === "party" || p.location === "dps")) ||
      (p.location === "base" && guild != null && p.guild === guild),
  );
}

// Reactive status for the UI (last successful sync, last error).
export const syncStatus = $state<{ lastSyncMs: number; lastError: string | null }>({
  lastSyncMs: 0,
  lastError: null,
});

let _timer: ReturnType<typeof setInterval> | null = null;

/// One poll: if auto-sync is on and a connection was saved, re-fetch the roster
/// and replace the server-sourced owned pals (manual/scanned pals are kept).
async function tick(): Promise<void> {
  const c = readServerConf();
  if (!c.autoSync || !hasSavedConnection(c)) return;
  try {
    await ensureCatalogs();
    const raw = await invoke<string>("fetch_server_roster", {
      url: c.url,
      token: c.token,
      fingerprint: c.fingerprint,
    });
    const roster = JSON.parse(raw) as ServerRoster;
    if (!Array.isArray(roster.pals) || !Array.isArray(roster.players)) {
      throw new Error("unexpected response from server (not a roster)");
    }
    const mapped = filterForOwner(roster, c.owner)
      .map(serverPalToOwned)
      .filter((o): o is OwnedPal => o !== null);
    replaceServerOwned(mapped);
    syncStatus.lastSyncMs = Date.now();
    syncStatus.lastError = null;
  } catch (e) {
    // Keep the last-known pals and try again next tick; surface the reason.
    syncStatus.lastError = String(e);
  }
}

/// Poll immediately (used right after the user enables auto-sync or connects).
export function pollNow(): void {
  void tick();
}

/// Start the 20s background poller. Idempotent. Call once, after the owned-pals
/// store has loaded, so the first sync doesn't race persisted pals.
export function initServerSync(): void {
  if (_timer) return;
  _timer = setInterval(() => void tick(), POLL_MS);
  void tick(); // initial sync on launch when already enabled + connected
}

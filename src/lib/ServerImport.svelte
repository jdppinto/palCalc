<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type {
    OwnedPal,
    PalEntry,
    PassiveEntry,
    ServerRoster,
    ServerPal,
  } from "./types";
  import { addManyOwned, clearAllOwned } from "./owned.svelte";

  // Persisted connection (this is the user's own machine; the token is a local
  // secret stored alongside the app's other local state).
  const LS_KEY = "palcalc.server";

  let url = $state("");
  let fingerprint = $state("");
  let token = $state("");
  let collapsed = $state(true);

  let status = $state<"idle" | "connecting" | "connected" | "error">("idle");
  let error = $state("");
  let roster = $state<ServerRoster | null>(null);
  let selectedOwner = $state<string>(""); // "" = everyone
  let replace = $state(true);
  let importResult = $state("");
  // Last-selected player, restored after a reconnect if that player still exists.
  let savedOwner = "";

  // Valid palCalc keys, for mapping the save's internal names.
  let validSpecies = $state<Set<string>>(new Set());
  let validPassives = $state<Set<string>>(new Set());
  let displayName = $state<Map<string, string>>(new Map());

  (function init() {
    try {
      const s = JSON.parse(localStorage.getItem(LS_KEY) || "{}");
      url = s.url || "";
      fingerprint = s.fingerprint || "";
      token = s.token || "";
      savedOwner = s.owner || "";
    } catch {
      /* ignore malformed saved state */
    }
    void loadKeys();
  })();

  function persist() {
    localStorage.setItem(
      LS_KEY,
      JSON.stringify({
        url: url.trim(),
        fingerprint: fingerprint.trim(),
        token: token.trim(),
        owner: selectedOwner,
      }),
    );
  }

  async function loadKeys() {
    try {
      const pals = await invoke<PalEntry[]>("list_pals");
      validSpecies = new Set(pals.map((p) => p.key));
      displayName = new Map(pals.map((p) => [p.key, p.name]));
      const pass = await invoke<PassiveEntry[]>("list_passives");
      validPassives = new Set(pass.map((p) => p.key));
    } catch (e) {
      console.error("failed to load pal/passive keys", e);
    }
  }

  const canConnect = $derived(
    !!url.trim() && !!fingerprint.trim() && !!token.trim(),
  );

  async function connect() {
    status = "connecting";
    error = "";
    roster = null;
    importResult = "";
    const u = url.trim();
    const fp = fingerprint.trim();
    const tok = token.trim();
    try {
      const raw = await invoke<string>("fetch_server_roster", {
        url: u,
        token: tok,
        fingerprint: fp,
      });
      const parsed = JSON.parse(raw) as ServerRoster;
      // Guard against a valid-but-unexpected body (schema drift, an error
      // object with a 2xx, etc.) so the reactive template can't throw later.
      if (!Array.isArray(parsed.pals) || !Array.isArray(parsed.players)) {
        throw new Error("unexpected response from server (not a roster)");
      }
      roster = parsed;
      status = "connected";
      // Restore the last-selected player if they're still in this roster.
      selectedOwner = parsed.players.some((p) => p.uid === savedOwner)
        ? savedOwner
        : "";
      persist();
    } catch (e) {
      status = "error";
      error = String(e);
    }
  }

  // Species/passive names in the save are Palworld internal keys, which are the
  // same keys palCalc uses. Strip alpha/raid prefixes as a fallback.
  const PREFIXES = ["BOSS_", "GYM_", "RAID_", "PREDATOR_"];
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

  function toOwned(p: ServerPal): OwnedPal | null {
    const species = normSpecies(p.species);
    if (!species) return null;
    return {
      species,
      label: displayName.get(species) || species,
      passives: p.passives.filter((k) => validPassives.has(k)),
      gender: p.gender === "Male" ? "Male" : p.gender === "Female" ? "Female" : null,
    };
  }

  const selectedGuild = $derived(
    roster && selectedOwner
      ? (roster.players.find((p) => p.uid === selectedOwner)?.guild ?? null)
      : null,
  );

  // A player's own palbox/party pals, plus their whole guild's base pals
  // (guild attribution is authoritative — includes guildmates' base workers).
  const selectedPals = $derived.by(() => {
    if (!roster) return [];
    if (selectedOwner === "") return roster.pals;
    const g = selectedGuild;
    return roster.pals.filter(
      (p) =>
        (p.owner === selectedOwner && (p.location === "palbox" || p.location === "party")) ||
        (p.location === "base" && g != null && p.guild === g),
    );
  });

  function ownerLabel(uid: string, name: string): string {
    const n = roster ? roster.pals.filter((p) => p.owner === uid).length : 0;
    return `${name || uid.slice(0, 8)} (${n})`;
  }

  function generatedAgo(): string {
    if (!roster) return "";
    const secs = Math.max(0, Math.floor(Date.now() / 1000) - roster.generated_at_unix);
    if (secs < 60) return `${secs}s ago`;
    if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
    return `${Math.floor(secs / 3600)}h ago`;
  }

  function importPals() {
    const mapped: OwnedPal[] = [];
    let skipped = 0;
    for (const p of selectedPals) {
      const o = toOwned(p);
      if (o) mapped.push(o);
      else skipped++;
    }
    if (replace) clearAllOwned();
    addManyOwned(mapped);
    importResult =
      `Imported ${mapped.length} pals` +
      (skipped ? ` (skipped ${skipped} unrecognized)` : "") +
      (replace ? ", replacing the previous list." : ", added to the list.");
  }
</script>

<section class="server">
  <button class="head" onclick={() => (collapsed = !collapsed)}>
    <span class="chev" class:open={!collapsed}>▶</span>
    Load from server
    {#if status === "connected"}<span class="ok">connected</span>{/if}
  </button>

  {#if !collapsed}
    <div class="body">
      <p class="hint">
        Pull your roster from a palcalc-server (no scanning). Paste the server
        URL, its certificate fingerprint, and your access token — all shared by
        the server's operator.
      </p>

      <label>Server URL
        <input class="field wide" type="text" bind:value={url} placeholder="https://host:8123/roster" />
      </label>
      <label>Certificate fingerprint (SHA-256)
        <input class="field wide" type="text" bind:value={fingerprint} placeholder="AA:BB:CC:…" />
      </label>
      <label>Access token
        <input class="field wide" type="password" bind:value={token} placeholder="shared token" />
      </label>

      <div class="row">
        <button class="primary" onclick={connect} disabled={status === "connecting" || !canConnect}>
          {status === "connecting" ? "Connecting…" : "Connect"}
        </button>
        {#if status === "error"}<span class="err">{error}</span>{/if}
      </div>

      {#if roster}
        <div class="loaded">
          <div class="meta">
            {roster.players.length} players · {roster.pals.length} pals · updated {generatedAgo()}
          </div>
          <label>Import pals of
            <select class="field" bind:value={selectedOwner} onchange={() => { savedOwner = selectedOwner; persist(); }}>
              <option value="">Everyone ({roster.pals.length})</option>
              {#each roster.players as pl (pl.uid)}
                <option value={pl.uid}>{ownerLabel(pl.uid, pl.name)}</option>
              {/each}
            </select>
          </label>
          <label class="check">
            <input type="checkbox" bind:checked={replace} />
            Replace current owned pals
          </label>
          <div class="row">
            <button class="primary" onclick={importPals} disabled={selectedPals.length === 0}>
              Import {selectedPals.length} pals
            </button>
            {#if importResult}<span class="ok">{importResult}</span>{/if}
          </div>
          <p class="note">
            Selecting a player imports their palbox and party plus their whole
            guild's base pals (including guildmates' base workers). "Everyone"
            imports the entire server roster.
          </p>
        </div>
      {/if}
    </div>
  {/if}
</section>

<style>
  .server {
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-raised);
    margin-bottom: 1rem;
  }
  .head {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.7rem 1rem;
    background: none;
    border: none;
    color: var(--text);
    font-weight: 600;
    cursor: pointer;
    text-align: left;
  }
  .chev {
    display: inline-block;
    transition: transform 0.15s;
    color: var(--text-dim);
    font-size: 0.7rem;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .ok {
    color: #3fb950;
    font-weight: 500;
    font-size: 0.85rem;
  }
  .err {
    color: #f85149;
    font-size: 0.85rem;
  }
  .body {
    padding: 0 1rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .hint,
  .note {
    color: var(--text-dim);
    font-size: 0.82rem;
    margin: 0;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  label.check {
    flex-direction: row;
    align-items: center;
    gap: 0.4rem;
    color: var(--text);
  }
  /* Class-based (not [type=…]) selectors: Svelte's scoper collapses attribute
     selectors like input[type="text"] to a bare scope class that then matches
     every scoped element — including the <select>. */
  .field {
    box-sizing: border-box;
    padding: 0.4rem 0.55rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    font: inherit;
  }
  /* Text fields fill the column (capped); the dropdown sizes to its content. */
  .field.wide {
    width: 100%;
    max-width: 28rem;
  }
  select.field {
    align-self: flex-start;
    max-width: 100%;
    cursor: pointer;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  button.primary {
    padding: 0.45rem 1rem;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    font-weight: 600;
  }
  button.primary:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .loaded {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    border-top: 1px solid var(--border);
    padding-top: 0.7rem;
  }
  .meta {
    font-size: 0.82rem;
    color: var(--text-dim);
  }
</style>

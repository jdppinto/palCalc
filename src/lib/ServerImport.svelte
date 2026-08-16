<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { OwnedPal, ServerRoster } from "./types";
  import { addManyOwned, clearAllOwned } from "./owned.svelte";
  import { toast } from "./toast.svelte";
  import {
    ensureCatalogs,
    filterForOwner,
    pollNow,
    serverPalToOwned,
    syncStatus,
  } from "./serverSync.svelte";

  // When embedded (e.g. in the Roster's "Add pals" panel) the outer collapse
  // header is dropped and the body is always shown.
  let { alwaysOpen = false }: { alwaysOpen?: boolean } = $props();

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
  // Re-fetch the roster every 20s and refresh server pals (see serverSync).
  let autoSync = $state(false);
  // Last-selected player, restored after a reconnect if that player still exists.
  let savedOwner = "";

  (function init() {
    try {
      const s = JSON.parse(localStorage.getItem(LS_KEY) || "{}");
      url = s.url || "";
      fingerprint = s.fingerprint || "";
      token = s.token || "";
      savedOwner = s.owner || "";
      autoSync = !!s.autoSync;
    } catch {
      /* ignore malformed saved state */
    }
    void ensureCatalogs();
    // Auto-connect when we already have saved connection details, so opening
    // "From server" reconnects immediately instead of needing a Connect click.
    if (url.trim() && fingerprint.trim() && token.trim()) void connect();
  })();

  function persist() {
    localStorage.setItem(
      LS_KEY,
      JSON.stringify({
        url: url.trim(),
        fingerprint: fingerprint.trim(),
        token: token.trim(),
        owner: selectedOwner,
        autoSync,
      }),
    );
  }

  // Toggle background auto-sync: persist the choice and, when turning it on,
  // kick a sync immediately instead of waiting for the first 20s tick.
  function onAutoSyncChange() {
    persist();
    if (autoSync) pollNow();
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

  // Mapping (save keys → palCalc keys) and owner filtering are shared with the
  // background poller so manual import and auto-sync agree exactly.
  const selectedPals = $derived(roster ? filterForOwner(roster, selectedOwner) : []);

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

  async function importPals() {
    await ensureCatalogs();
    const mapped: OwnedPal[] = [];
    let skipped = 0;
    for (const p of selectedPals) {
      const o = serverPalToOwned(p);
      if (o) mapped.push(o);
      else skipped++;
    }
    if (replace) clearAllOwned();
    addManyOwned(mapped);
    importResult =
      `Imported ${mapped.length} pals` +
      (skipped ? ` (skipped ${skipped} unrecognized)` : "") +
      (replace ? ", replacing the previous list." : ", added to the list.");
    toast.success(
      `Imported ${mapped.length} pals into your roster${skipped ? ` (${skipped} skipped)` : ""}.`,
    );
  }
</script>

<section class="server">
  {#if !alwaysOpen}
    <button class="head" onclick={() => (collapsed = !collapsed)}>
      <span class="chev" class:open={!collapsed}>▶</span>
      Load from server
      {#if status === "connected"}<span class="ok">connected</span>{/if}
    </button>
  {/if}

  {#if alwaysOpen || !collapsed}
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
          <label class="check">
            <input type="checkbox" bind:checked={autoSync} onchange={onAutoSyncChange} />
            Auto-sync every 20s
          </label>
          {#if autoSync}
            <p class="note">
              Re-checks the server every 20s and refreshes your palbox pals
              automatically (hand-added and scanned pals are kept). Keeps running
              while the app is open, and resumes next time you open it.
              {#if syncStatus.lastError}<br /><span class="err">last sync failed: {syncStatus.lastError}</span>{/if}
            </p>
          {/if}
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
    color: var(--success);
    font-weight: 500;
    font-size: 0.85rem;
  }
  .err {
    color: var(--danger);
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

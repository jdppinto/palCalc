<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import type { PalEntry, PassiveEntry, Gender, PalLocation } from "./types";
  import {
    ownedStore,
    addOwnedPal,
    removeOwnedAt,
    clearAllOwned,
  } from "./owned.svelte";
  import PalSelect from "./PalSelect.svelte";
  import PassivePicker from "./PassivePicker.svelte";
  import ServerImport from "./ServerImport.svelte";
  import PalboxScanner from "./PalboxScanner.svelte";

  let pals = $state<PalEntry[]>([]);
  let passives = $state<PassiveEntry[]>([]);
  let source = $state<"" | "server" | "scan" | "manual">("");

  // Manual add-pal form.
  let newSpecies = $state<string | null>(null);
  let newPassives = $state<string[]>([]);
  let newGender = $state<Gender | null>(null);

  onMount(async () => {
    try {
      pals = await invoke<PalEntry[]>("list_pals");
      passives = await invoke<PassiveEntry[]>("list_passives");
    } catch (e) {
      console.error("failed to load pal/passive catalogs", e);
    }
  });

  const palByKey = $derived(new Map(pals.map((p) => [p.key, p])));
  const passiveName = (k: string) => passives.find((p) => p.key === k)?.name ?? k;
  const iconOf = (species: string) => palByKey.get(species)?.icon ?? null;
  const nameOf = (species: string) => palByKey.get(species)?.name ?? species;
  const sym = (g: Gender | null) => (g === "Male" ? "♂" : g === "Female" ? "♀" : "");

  // Location filter (only meaningful for server-imported pals, which carry a
  // location). Filter against the full list; map back by reference to remove.
  let locFilter = $state<"" | PalLocation>("");
  const hasLocations = $derived(ownedStore.list.some((p) => p.location));
  const shown = $derived(
    locFilter
      ? ownedStore.list.filter((p) => p.location === locFilter)
      : ownedStore.list,
  );

  function toggle(s: typeof source) {
    source = source === s ? "" : s;
  }

  function addManual() {
    if (!newSpecies) return;
    const n = ownedStore.list.filter((p) => p.species === newSpecies).length + 1;
    addOwnedPal({
      species: newSpecies,
      label: `${nameOf(newSpecies)} #${n}`,
      passives: newPassives,
      gender: newGender,
      source: "manual",
    });
    newSpecies = null;
    newPassives = [];
    newGender = null;
  }

  function removeAll() {
    if (confirm("Remove all pals from your roster?")) clearAllOwned();
  }
</script>

<section class="roster">
  <header class="rhead">
    <h2>
      Roster
      <span class="count">
        · {shown.length}{#if locFilter} of {ownedStore.list.length}{/if} pals
      </span>
    </h2>
    <div class="rhead-actions">
      {#if hasLocations}
        <label class="filter">
          Location
          <select bind:value={locFilter}>
            <option value="">All</option>
            <option value="palbox">Palbox</option>
            <option value="party">Party</option>
            <option value="base">Base</option>
            <option value="dps">Dimensional</option>
          </select>
        </label>
      {/if}
      {#if ownedStore.list.length}
        <button class="danger" onclick={removeAll}>Remove all</button>
      {/if}
    </div>
  </header>

  <!-- Add controls live at the top so they're reachable without scrolling
       past a large roster. -->
  <div class="add">
    <div class="addhead">
      <h3>Add pals</h3>
      <div class="sources">
        <button class="src" class:active={source === "server"} onclick={() => toggle("server")}>From server</button>
        <button class="src" class:active={source === "scan"} onclick={() => toggle("scan")}>Scan game screen</button>
        <button class="src" class:active={source === "manual"} onclick={() => toggle("manual")}>Add manually</button>
      </div>
    </div>

    {#if source === "server"}
      <ServerImport alwaysOpen />
    {:else if source === "scan"}
      <PalboxScanner />
    {:else if source === "manual"}
      <div class="manual">
        <PalSelect {pals} bind:value={newSpecies} label="Species" />
        <PassivePicker {passives} bind:selected={newPassives} max={4} label="Its passives (up to 4)" />
        <div class="genders">
          <span class="glabel">Gender</span>
          <label><input type="radio" name="rostergender" checked={newGender === null} onchange={() => (newGender = null)} /> any</label>
          <label><input type="radio" name="rostergender" checked={newGender === "Male"} onchange={() => (newGender = "Male")} /> ♂</label>
          <label><input type="radio" name="rostergender" checked={newGender === "Female"} onchange={() => (newGender = "Female")} /> ♀</label>
        </div>
        <button class="primary" onclick={addManual} disabled={!newSpecies}>Add pal</button>
      </div>
    {/if}
  </div>

  {#if !ownedStore.list.length}
    <p class="empty">
      No pals in your roster yet. Add some above — import from your
      palcalc-server, scan the game screen, or add them by hand.
    </p>
  {:else if !shown.length}
    <p class="empty">No pals match this filter.</p>
  {:else}
    <div class="grid">
      {#each shown as p (p)}
        <div class="card">
          <button class="rm" title="Remove" onclick={() => removeOwnedAt(ownedStore.list.indexOf(p))}>✕</button>
          {#if iconOf(p.species)}
            <img class="icon" src={"/icons/" + iconOf(p.species)} alt={nameOf(p.species)} />
          {/if}
          <div class="name">{nameOf(p.species)} <span class="g">{sym(p.gender)}</span></div>
          {#if p.level || (p.location && p.location !== "unknown")}
            <div class="meta">
              {#if p.level}<span class="lv">Lv {p.level}</span>{/if}
              {#if p.location && p.location !== "unknown"}
                <span class="loc {p.location}">{p.location}</span>
              {/if}
            </div>
          {/if}
          <div class="passives">
            {#if p.passives.length}
              {#each p.passives as k}<span class="pv">{passiveName(k)}</span>{/each}
            {:else}
              <span class="pv none">no passives</span>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .roster {
    max-width: 1100px;
    margin: 0 auto;
    padding: 1.25rem 1.5rem;
  }
  .rhead {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
  }
  h2 {
    margin: 0;
    font-size: 1.1rem;
  }
  .count {
    color: var(--text-dim);
    font-weight: 400;
  }
  .rhead-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  .filter {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.8rem;
    color: var(--text-dim);
  }
  .filter select {
    padding: 0.3rem 0.5rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }

  .add {
    margin-top: 0.9rem;
    padding-bottom: 1.1rem;
    border-bottom: 1px solid var(--border);
  }
  .addhead {
    display: flex;
    align-items: center;
    gap: 1rem;
    flex-wrap: wrap;
  }
  h3 {
    margin: 0;
    font-size: 0.95rem;
  }
  .sources {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .src {
    padding: 0.4rem 0.85rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
  }
  .src:hover {
    background: var(--bg-hover);
    color: var(--text);
  }
  .src.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
    font-weight: 600;
  }
  .manual {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    margin-top: 0.9rem;
    max-width: 28rem;
  }
  .genders {
    display: flex;
    align-items: center;
    gap: 0.9rem;
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  .genders label {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    cursor: pointer;
  }
  .glabel {
    color: var(--text-dim);
  }
  button.primary {
    align-self: flex-start;
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
  button.danger {
    padding: 0.35rem 0.7rem;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
  }
  button.danger:hover {
    border-color: var(--danger);
    color: var(--danger);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 0.6rem;
    margin-top: 1.1rem;
  }
  .card {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.25rem;
    padding: 0.6rem 0.5rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 10px;
    text-align: center;
  }
  .rm {
    position: absolute;
    top: 0.25rem;
    right: 0.25rem;
    width: 1.25rem;
    height: 1.25rem;
    line-height: 1;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
  }
  .rm:hover {
    background: var(--bg-hover);
    color: var(--danger);
  }
  .icon {
    width: 48px;
    height: 48px;
    object-fit: contain;
  }
  .name {
    font-size: 0.85rem;
    font-weight: 600;
  }
  .g {
    color: var(--text-dim);
    font-weight: 400;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.25rem;
    font-size: 0.66rem;
  }
  .lv {
    color: var(--text-dim);
  }
  .loc {
    padding: 0.02rem 0.3rem;
    border-radius: 999px;
    text-transform: capitalize;
    background: var(--bg-hover);
    color: var(--text-dim);
  }
  .loc.palbox {
    background: var(--info-soft);
    color: var(--info);
  }
  .loc.party {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .loc.base {
    background: var(--success-soft);
    color: var(--success);
  }
  .loc.dps {
    background: var(--purple-soft);
    color: var(--purple);
  }
  .passives {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.2rem;
  }
  .pv {
    font-size: 0.68rem;
    padding: 0.05rem 0.3rem;
    border-radius: 4px;
    background: var(--bg-hover);
    color: var(--text-dim);
  }
  .pv.none {
    background: none;
    font-style: italic;
  }
  .empty {
    color: var(--text-dim);
    margin: 1.1rem 0 0;
  }
  .add :global(.server) {
    margin-top: 0.9rem;
  }
</style>

<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { addOwnedPal, ownedStore, removeOwnedAt } from "./owned.svelte";
  import PalSelect from "./PalSelect.svelte";
  import PassivePicker from "./PassivePicker.svelte";
  import type {
    Gender,
    OwnedPal,
    PalEntry,
    PassiveEntry,
    PlanOutcome,
    PlanStats,
    Route,
    RouteNode,
  } from "./types";

  let { onShowTree }: { onShowTree: (route: Route) => void } = $props();

  let pals = $state<PalEntry[]>([]);
  let passives = $state<PassiveEntry[]>([]);

  let target = $state<string | null>(null);
  let desired = $state<string[]>([]);
  let assumeWild = $state(false);
  let maxSteps = $state(500);
  let reversers = $state(0);

  let newSpecies = $state<string | null>(null);
  let newPassives = $state<string[]>([]);
  let newGender = $state<Gender | null>(null);

  let routes = $state<Route[] | null>(null);
  let stats = $state<PlanStats | null>(null);
  let planning = $state(false);
  let error = $state<string | null>(null);

  onMount(async () => {
    const all = await invoke<PalEntry[]>("list_pals");
    pals = all.filter((p) => p.icon !== null);
    passives = await invoke<PassiveEntry[]>("list_passives");
  });

  function palName(key: string): string {
    return pals.find((p) => p.key === key)?.name ?? key;
  }

  function passiveName(key: string): string {
    return passives.find((p) => p.key === key)?.name ?? key;
  }

  function addOwned() {
    if (!newSpecies) return;
    addOwnedPal({
      species: newSpecies,
      label: `${palName(newSpecies)} #${ownedStore.list.length + 1}`,
      passives: newPassives,
      gender: newGender,
    });
    newSpecies = null;
    newPassives = [];
    newGender = null;
  }

  function genderSymbol(g: Gender | null): string {
    return g === "Male" ? " ♂" : g === "Female" ? " ♀" : "";
  }

  async function planRoutes() {
    if (!target) return;
    planning = true;
    error = null;
    try {
      const out = await invoke<PlanOutcome>("plan", {
        req: {
          target,
          desired_passives: desired,
          owned: ownedStore.list,
          assume_wild: assumeWild,
          max_steps: maxSteps,
          reversers,
        },
      });
      routes = out.routes;
      stats = out.stats;
    } catch (e) {
      error = String(e);
      routes = null;
      stats = null;
    } finally {
      planning = false;
    }
  }

</script>

{#snippet tree(node: RouteNode, gender: Gender | null)}
  <li>
    <div class="node" class:leaf={node.owned !== null}>
      {#if node.icon}
        <img src={"/icons/" + node.icon} alt="" />
      {/if}
      <span class="name">{node.name}{genderSymbol(gender)}</span>
      {#if node.owned === "wild"}
        <span class="tag wild">wild catch</span>
      {:else if node.owned !== null}
        <span class="tag owned">{node.owned}</span>
      {:else}
        <span class="tag bred">bred</span>
      {/if}
      {#each node.passives as p (p)}
        <span class="tag passive">{p}</span>
      {/each}
    </div>
    {#if node.parents.length === 2}
      <ul>
        {@render tree(node.parents[0], node.gender_a)}
        {@render tree(node.parents[1], node.gender_b)}
      </ul>
    {/if}
  </li>
{/snippet}

<section>
  <div class="config">
    <div class="row">
      <PalSelect {pals} bind:value={target} label="Target pal" />
      <label class="wild">
        <input type="checkbox" bind:checked={assumeWild} />
        assume wild catches
      </label>
      <label class="rev">
        reversers
        <input type="number" min="0" bind:value={reversers} />
      </label>
      <label class="depth">
        max steps
        <input type="number" min="1" bind:value={maxSteps} />
      </label>
    </div>

    {#if maxSteps > 500}
      <p class="warn">
        ⚠ {maxSteps}-step budget — with many desired passives and owned pals
        this can get expensive and your PC will suffer. (Often the search
        converges early instead — the stats line will tell you which happened.)
      </p>
    {/if}

    <PassivePicker {passives} bind:selected={desired} label="Desired passives (up to 8)" />

    <details open={ownedStore.list.length > 0}>
      <summary>Owned pals ({ownedStore.list.length})</summary>
      <div class="owned-add">
        <PalSelect {pals} bind:value={newSpecies} label="Species" />
        <div class="owned-passives">
        <PassivePicker
          {passives}
          bind:selected={newPassives}
          max={4}
          label="Its passives (up to 4)"
        />
        </div>
        <div class="owned-gender">
          <label>
            <input type="radio" name="newGender" checked={newGender === null} onclick={() => newGender = null} />
            any
          </label>
          <label>
            <input type="radio" name="newGender" checked={newGender === "Male"} onclick={() => newGender = "Male"} />
            ♂
          </label>
          <label>
            <input type="radio" name="newGender" checked={newGender === "Female"} onclick={() => newGender = "Female"} />
            ♀
          </label>
        </div>
        <button class="add" onclick={addOwned} disabled={!newSpecies}>Add</button>
      </div>
      {#if ownedStore.list.length > 0}
        <ul class="owned-list">
          {#each ownedStore.list as o, i (i)}
            <li>
              <span>{o.label}{genderSymbol(o.gender)}</span>
              <span class="dim">
                {o.passives.length
                  ? o.passives.map(passiveName).join(", ")
                  : "no passives"}
              </span>
              <button onclick={() => removeOwnedAt(i)} title="Remove">✕</button>
            </li>
          {/each}
        </ul>
      {/if}
    </details>

    <button class="plan" onclick={planRoutes} disabled={!target || planning}>
      {planning ? "Planning…" : "Plan routes"}
    </button>
  </div>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if routes !== null}
    {#if stats}
      <p class="stats">
        {routes.length} route{routes.length === 1 ? "" : "s"} ·
        {stats.states.toLocaleString()} states explored in {stats.elapsed_ms} ms ·
        {#if stats.converged}
          search fully exhausted after {stats.rounds} round{stats.rounds === 1
            ? ""
            : "s"} — a higher step budget can't change these results
        {:else}
          explored everything within the {stats.max_steps}-step budget ({stats.rounds}
          round{stats.rounds === 1 ? "" : "s"}) — raising max steps may find
          more or better routes
        {/if}
      </p>
    {/if}
    {#if routes.length === 0}
      <p class="hint">
        No route found — add owned pals, enable wild catches, or raise max steps.
      </p>
    {:else}
      <div class="routes">
        {#each routes as r, i (i)}
          <div class="route">
            <header>
              <strong>Route {i + 1}</strong>
              <span>{r.steps} step{r.steps === 1 ? "" : "s"}</span>
              {#if r.reversers_used}
                <span class="tag rev">♻{r.reversers_used}</span>
              {/if}
              {#each r.covered as p (p)}
                <span class="tag passive">✓ {p}</span>
              {/each}
              {#each r.missing as p (p)}
                <span class="tag missing">✗ {p}</span>
              {/each}
              <button class="show-tree" onclick={() => onShowTree(r)}>
                Show tree →
              </button>
            </header>
            <ul class="tree-root">
              {@render tree(r.root, null)}
            </ul>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</section>

<style>
  section {
    max-width: 860px;
    margin: 0 auto;
    padding: 1.5rem;
  }

  .config {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .row {
    display: flex;
    align-items: flex-end;
    gap: 1rem;
  }

  .wild {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding-bottom: 0.6rem;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .rev,
  .depth {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding-bottom: 0.35rem;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .rev input {
    width: 3.2rem;
    padding: 0.3rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
  }

  .depth input {
    width: 3.2rem;
    padding: 0.3rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
  }

  details {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.6rem 0.9rem;
  }

  summary {
    cursor: pointer;
    color: var(--text-dim);
  }

  .owned-add {
    display: flex;
    align-items: flex-end;
    gap: 0.75rem;
    margin-top: 0.75rem;
  }

  .owned-passives {
    flex: 1.4;
  }

  .owned-gender {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    padding-bottom: 0.15rem;
  }

  .owned-gender label {
    display: flex;
    align-items: center;
    gap: 0.15rem;
    cursor: pointer;
    font-size: 0.9rem;
  }

  .add {
    padding: 0.55rem 1rem;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
  }

  .owned-list {
    list-style: none;
    margin: 0.75rem 0 0;
    padding: 0;
  }

  .owned-list li {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.3rem 0;
  }

  .owned-list button {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
  }

  .dim {
    color: var(--text-dim);
    font-size: 0.85rem;
  }

  .plan {
    align-self: flex-start;
    padding: 0.6rem 1.4rem;
    background: var(--accent);
    color: #1a1408;
    font-weight: 600;
    border: none;
    border-radius: 8px;
    cursor: pointer;
  }

  .plan:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .routes {
    margin-top: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .route {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 1rem;
  }

  .route header {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
    margin-bottom: 0.75rem;
  }

  .route header span {
    color: var(--text-dim);
  }

  .show-tree {
    margin-left: auto;
    padding: 0.3rem 0.8rem;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    font-size: 0.85rem;
  }

  .show-tree:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  ul.tree-root,
  .route ul {
    list-style: none;
    margin: 0;
    padding-left: 1.4rem;
    border-left: 1px solid var(--border);
  }

  ul.tree-root {
    border-left: none;
    padding-left: 0;
  }

  .node {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0;
    flex-wrap: wrap;
  }

  .node img {
    width: 30px;
    height: 30px;
  }

  .name {
    font-weight: 500;
  }

  .tag {
    font-size: 0.75rem;
    padding: 0.1rem 0.5rem;
    border-radius: 999px;
  }

  .tag.owned {
    background: rgba(34, 197, 94, 0.15);
    color: #4ade80;
  }

  .tag.wild {
    background: rgba(59, 130, 246, 0.15);
    color: #60a5fa;
  }

  .tag.bred {
    background: var(--bg-hover);
    color: var(--text-dim);
  }

  .tag.passive {
    background: var(--accent-soft);
    color: var(--accent);
  }

  .tag.missing {
    background: rgba(239, 68, 68, 0.15);
    color: #f87171;
  }

  .tag.rev {
    background: rgba(168, 85, 247, 0.15);
    color: #c084fc;
  }

  .warn {
    margin: -0.4rem 0 0;
    color: var(--accent);
    font-size: 0.85rem;
  }

  .stats {
    margin: 1.25rem 0 0;
    color: var(--text-dim);
    font-size: 0.85rem;
  }

  .error {
    margin-top: 1.5rem;
    color: #ef4444;
  }

  .hint {
    margin-top: 1.5rem;
    color: var(--text-dim);
  }
</style>

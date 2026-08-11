<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { ownedStore } from "./owned.svelte";
  import { isBookmarked, toggleBookmark } from "./bookmarks.svelte";
  import Combobox from "./Combobox.svelte";
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

  let {
    onShowTree,
    onManageRoster,
  }: { onShowTree: (route: Route) => void; onManageRoster?: () => void } = $props();

  let pals = $state<PalEntry[]>([]);
  let passives = $state<PassiveEntry[]>([]);

  let target = $state<string | null>(null);
  let desired = $state<string[]>([]);
  let assumeWild = $state(false);
  const WARN_STEP_THRESHOLD = 500;
  let maxSteps = $state(WARN_STEP_THRESHOLD);
  let reversers = $state(0);

  let routes = $state<Route[] | null>(null);
  let stats = $state<PlanStats | null>(null);
  let planning = $state(false);
  let error = $state<string | null>(null);

  // Session-only history of completed calculations, so a result isn't lost
  // when the user runs another one before bookmarking. Kept in memory (this
  // component stays mounted across tab switches); bookmarks are for permanence.
  interface CalcSnapshot {
    id: string;
    label: string;
    // inputs
    target: string;
    desired: string[];
    assumeWild: boolean;
    maxSteps: number;
    reversers: number;
    // outputs
    routes: Route[];
    stats: PlanStats | null;
  }
  let history = $state<CalcSnapshot[]>([]);
  let currentId = $state<string | null>(null);
  const HISTORY_CAP = 10;

  // Same inputs? (desired compared order-insensitively.) Used to avoid a
  // duplicate entry when the same calculation is re-run.
  function sameInputs(s: CalcSnapshot): boolean {
    return (
      s.target === target &&
      s.assumeWild === assumeWild &&
      s.maxSteps === maxSteps &&
      s.reversers === reversers &&
      s.desired.length === desired.length &&
      [...s.desired].sort().join() === [...desired].sort().join()
    );
  }

  function restore(s: CalcSnapshot) {
    // Reassign the bound input state (the Combobox pickers and inputs follow);
    // owned pals are intentionally NOT restored — they live in the shared
    // ownedStore, so the result reflects the owned set at compute time and a
    // re-plan uses the current one.
    target = s.target;
    desired = [...s.desired];
    assumeWild = s.assumeWild;
    maxSteps = s.maxSteps;
    reversers = s.reversers;
    routes = s.routes;
    stats = s.stats;
    error = null;
    currentId = s.id;
  }

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

  function genderSymbol(g: Gender | null): string {
    return g === "Male" ? " ♂" : g === "Female" ? " ♀" : "";
  }

  async function planRoutes() {
    if (!target) return;
    // Capture inputs up front: after the await, TS no longer narrows the
    // component-level `target` away from null, and this is the exact input set
    // the result belongs to.
    const tgt = target;
    const desiredSnap = [...desired];
    planning = true;
    error = null;
    try {
      const out = await invoke<PlanOutcome>("plan", {
        req: {
          target: tgt,
          desired_passives: desiredSnap,
          owned: ownedStore.list,
          assume_wild: assumeWild,
          max_steps: Number.isFinite(maxSteps) ? maxSteps : 500,
          reversers,
        },
      });
      routes = out.routes;
      stats = out.stats;
      // Record this completed calculation so it can be recalled later. Replace
      // the top entry instead of duplicating when the inputs are unchanged.
      const passiveLabel = desiredSnap.length ? desiredSnap.map(passiveName).join(", ") : "none";
      const snap: CalcSnapshot = {
        id: crypto.randomUUID(),
        label: `${palName(tgt)} · ${passiveLabel} · ${out.routes.length} route${out.routes.length === 1 ? "" : "s"}`,
        target: tgt,
        desired: desiredSnap,
        assumeWild,
        maxSteps,
        reversers,
        routes: out.routes,
        stats: out.stats,
      };
      const rest = history[0] && sameInputs(history[0]) ? history.slice(1) : history;
      history = [snap, ...rest].slice(0, HISTORY_CAP);
      currentId = snap.id;
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
    <div class="node">
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
      <Combobox options={pals} bind:value={target} label="Target pal" />
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

    {#if maxSteps > WARN_STEP_THRESHOLD}
      <p class="warn">
        ⚠ {maxSteps}-step budget — with many desired passives and owned pals
        this can get expensive and your PC will suffer. (Often the search
        converges early instead — the stats line will tell you which happened.)
      </p>
    {/if}

    <Combobox options={passives} bind:selected={desired} multiple max={8} showRank label="Desired passives (up to 8)" />

    <div class="roster-summary">
      <span class="dim">
        Planning from your roster: <strong>{ownedStore.list.length}</strong>
        pal{ownedStore.list.length === 1 ? "" : "s"}
      </span>
      <button class="manage" onclick={() => onManageRoster?.()}>Manage in Roster →</button>
    </div>

    <button class="plan" onclick={planRoutes} disabled={!target || planning}>
      {planning ? "Planning…" : "Plan routes"}
    </button>
  </div>

  {#if history.length > 0}
    <details class="history">
      <summary>Recent calculations ({history.length})</summary>
      <p class="history-hint">
        Session history — click one to restore its inputs and results. Bookmark
        a route to keep it permanently.
      </p>
      <ul>
        {#each history as h (h.id)}
          <li>
            <button
              class="restore"
              class:active={h.id === currentId}
              title="Restore this calculation"
              onclick={() => restore(h)}
            >
              {h.label}
            </button>
          </li>
        {/each}
      </ul>
      <button class="history-clear" onclick={() => { history = []; currentId = null; }}>
        Clear history
      </button>
    </details>
  {/if}

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
              <button
                class="bookmark"
                class:saved={isBookmarked(r)}
                title={isBookmarked(r) ? "Remove bookmark" : "Bookmark this route"}
                onclick={() => toggleBookmark(r)}
              >
                {isBookmarked(r) ? "★ Saved" : "☆ Bookmark"}
              </button>
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

  .roster-summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-top: 0.25rem;
    padding: 0.5rem 0.75rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .manage {
    flex-shrink: 0;
    padding: 0.35rem 0.8rem;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text);
    cursor: pointer;
    font: inherit;
  }
  .manage:hover {
    border-color: var(--accent);
    color: var(--accent);
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

  /* Bookmark sits just left of Show tree; it carries the margin-left:auto so
     the pair floats to the right edge together. */
  .bookmark {
    margin-left: auto;
    padding: 0.3rem 0.8rem;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  .bookmark:hover {
    color: var(--accent);
    border-color: var(--accent);
  }
  .bookmark.saved {
    color: var(--accent);
  }
  /* When saved, hovering hints removal rather than re-adding. */
  .bookmark.saved:hover {
    color: #d0652a;
    border-color: #d0652a;
  }
  .show-tree {
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

  .history {
    margin-top: 1rem;
    padding: 0.5rem 0.75rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .history summary {
    cursor: pointer;
    font-size: 0.9rem;
  }
  .history-hint {
    margin: 0.4rem 0;
    font-size: 0.72rem;
    color: var(--text-dim);
  }
  .history ul {
    list-style: none;
    margin: 0.25rem 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .history .restore {
    width: 100%;
    text-align: left;
    padding: 0.3rem 0.5rem;
    background: none;
    border: 1px solid transparent;
    border-radius: 6px;
    color: var(--text);
    font-size: 0.8rem;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .history .restore:hover {
    border-color: var(--border);
    color: var(--accent);
  }
  .history .restore.active {
    border-color: var(--accent);
    color: var(--accent);
  }
  .history-clear {
    margin-top: 0.4rem;
    padding: 0.2rem 0.5rem;
    background: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-dim);
    font-size: 0.72rem;
    cursor: pointer;
  }
  .history-clear:hover {
    color: #d0652a;
    border-color: #d0652a;
  }
</style>

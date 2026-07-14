<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import PalSelect from "./PalSelect.svelte";
  import type { BreedResult, Gender, PalEntry } from "./types";

  let pals = $state<PalEntry[]>([]);
  let a = $state<string | null>(null);
  let b = $state<string | null>(null);
  let results = $state<BreedResult[]>([]);
  let error = $state<string | null>(null);

  onMount(async () => {
    const all = await invoke<PalEntry[]>("list_pals");
    // Icon presence ≈ actually obtainable: the 34 icon-less tribes are
    // non-breedable boss/unreleased pals (see PLAN.md).
    pals = all.filter((p) => p.icon !== null);
  });

  $effect(() => {
    if (!a || !b) {
      results = [];
      return;
    }
    invoke<BreedResult[]>("calculate_simple", { a, b })
      .then((r) => {
        results = r;
        error = null;
      })
      .catch((e) => {
        error = String(e);
        results = [];
      });
  });

  function swap() {
    [a, b] = [b, a];
  }

  function palName(key: string | null): string {
    return pals.find((p) => p.key === key)?.name ?? key ?? "?";
  }

  function genderSymbol(g: Gender): string {
    return g === "Male" ? "♂" : "♀";
  }
</script>

<section>
  <div class="parents">
    <PalSelect {pals} bind:value={a} label="Parent A" />
    <button class="swap" onclick={swap} title="Swap parents">⇄</button>
    <PalSelect {pals} bind:value={b} label="Parent B" />
  </div>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if results.length > 0}
    <div class="results">
      {#each results as r (r.child + (r.gender_a ?? ""))}
        <div class="card">
          {#if r.icon}
            <img src={"/icons/" + r.icon} alt={r.name} />
          {/if}
          <div>
            <h2>{r.name}</h2>
            {#if r.gender_a || r.gender_b}
              <p class="gender-req">
                requires
                {palName(a)}
                {r.gender_a ? genderSymbol(r.gender_a) : ""}
                ×
                {palName(b)}
                {r.gender_b ? genderSymbol(r.gender_b) : ""}
              </p>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {:else if a && b && !error}
    <p class="hint">No result</p>
  {:else}
    <p class="hint">Pick two parents to see their child.</p>
  {/if}
</section>

<style>
  section {
    max-width: 720px;
    margin: 0 auto;
    padding: 1.5rem;
  }

  .parents {
    display: flex;
    align-items: flex-end;
    gap: 0.75rem;
  }

  .swap {
    padding: 0.55rem 0.8rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    font-size: 1rem;
  }

  .swap:hover {
    background: var(--bg-hover);
  }

  .results {
    margin-top: 2rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .card {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 1rem;
    background: var(--bg-raised);
    border: 1px solid var(--accent);
    border-radius: 12px;
  }

  .card img {
    width: 64px;
    height: 64px;
  }

  .card h2 {
    margin: 0;
    font-size: 1.2rem;
  }

  .gender-req {
    margin: 0.25rem 0 0;
    color: var(--text-dim);
    font-size: 0.9rem;
  }

  .error {
    margin-top: 1.5rem;
    color: #ef4444;
  }

  .hint {
    margin-top: 2rem;
    color: var(--text-dim);
    text-align: center;
  }
</style>

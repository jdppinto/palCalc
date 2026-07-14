<script lang="ts">
  import type { PassiveEntry } from "./types";

  let {
    passives,
    selected = $bindable([]),
    max = 8,
    label,
  }: {
    passives: PassiveEntry[];
    selected: string[];
    max?: number;
    label: string;
  } = $props();

  let query = $state("");
  let open = $state(false);

  const filtered = $derived(
    passives.filter(
      (p) =>
        !selected.includes(p.key) &&
        p.name.toLowerCase().includes(query.trim().toLowerCase()),
    ),
  );

  function add(key: string) {
    if (selected.length < max) {
      selected = [...selected, key];
    }
    query = "";
    open = false;
  }

  function remove(key: string) {
    selected = selected.filter((k) => k !== key);
  }

  function nameOf(key: string): string {
    return passives.find((p) => p.key === key)?.name ?? key;
  }
</script>

<div class="picker">
  <span class="label">{label}</span>
  <div class="chips">
    {#each selected as key (key)}
      <button class="chip" onclick={() => remove(key)} title="Remove">
        {nameOf(key)} ✕
      </button>
    {/each}
    {#if selected.length < max}
      <input
        placeholder={selected.length ? "Add…" : "Type to search passives…"}
        bind:value={query}
        onfocus={() => (open = true)}
        onblur={() => setTimeout(() => (open = false), 150)}
      />
    {/if}
  </div>
  {#if open && filtered.length > 0}
    <ul>
      {#each filtered.slice(0, 40) as p (p.key)}
        <li>
          <button type="button" onmousedown={() => add(p.key)}>
            <span>{p.name}</span>
            <span class="rank">{p.rank > 0 ? "+" : ""}{p.rank}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .picker {
    position: relative;
  }

  .label {
    display: block;
    font-size: 0.8rem;
    color: var(--text-dim);
    margin-bottom: 0.3rem;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    padding: 0.35rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    min-height: 2.6rem;
  }

  .chip {
    padding: 0.25rem 0.55rem;
    background: var(--accent-soft);
    color: var(--accent);
    border: none;
    border-radius: 999px;
    cursor: pointer;
    font-size: 0.85rem;
  }

  input {
    flex: 1;
    min-width: 140px;
    background: none;
    border: none;
    outline: none;
    color: var(--text);
    padding: 0.25rem;
  }

  ul {
    position: absolute;
    z-index: 10;
    top: 100%;
    left: 0;
    right: 0;
    margin: 4px 0 0;
    padding: 4px;
    list-style: none;
    max-height: 260px;
    overflow-y: auto;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  li button {
    display: flex;
    justify-content: space-between;
    width: 100%;
    padding: 0.4rem 0.6rem;
    background: none;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
  }

  li button:hover {
    background: var(--bg-hover);
  }

  .rank {
    color: var(--text-dim);
  }
</style>

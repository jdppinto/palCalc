<script lang="ts">
  import type { PalEntry } from "./types";

  let {
    pals,
    value = $bindable(null),
    label,
  }: { pals: PalEntry[]; value: string | null; label: string } = $props();

  let query = $state("");
  let open = $state(false);

  const selected = $derived(pals.find((p) => p.key === value) ?? null);
  const filtered = $derived(
    query.trim()
      ? pals.filter((p) =>
          p.name.toLowerCase().includes(query.trim().toLowerCase()),
        )
      : pals,
  );

  function choose(p: PalEntry) {
    value = p.key;
    query = "";
    open = false;
  }
</script>

<div class="pal-select">
  <span class="label">{label}</span>
  <div class="control">
    {#if selected?.icon && !open}
      <img class="selected-icon" src={"/icons/" + selected.icon} alt="" />
    {/if}
    <input
      placeholder={selected ? selected.name : "Type to search…"}
      class:has-icon={selected?.icon && !open}
      bind:value={query}
      onfocus={() => (open = true)}
      onblur={() => setTimeout(() => (open = false), 150)}
    />
  </div>
  {#if open}
    <ul>
      {#each filtered.slice(0, 60) as p (p.key)}
        <li>
          <button type="button" onmousedown={() => choose(p)}>
            {#if p.icon}<img src={"/icons/" + p.icon} alt="" />{/if}
            <span>{p.name}</span>
          </button>
        </li>
      {/each}
      {#if filtered.length === 0}
        <li class="empty">No match</li>
      {/if}
    </ul>
  {/if}
</div>

<style>
  .pal-select {
    position: relative;
    flex: 1;
    min-width: 220px;
  }

  .label {
    display: block;
    font-size: 0.8rem;
    color: var(--text-dim);
    margin-bottom: 0.3rem;
  }

  .control {
    position: relative;
  }

  .selected-icon {
    position: absolute;
    left: 8px;
    top: 50%;
    transform: translateY(-50%);
    width: 26px;
    height: 26px;
    pointer-events: none;
  }

  input {
    width: 100%;
    padding: 0.6rem 0.75rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text);
    outline: none;
  }

  input.has-icon {
    padding-left: 42px;
  }

  input:focus {
    border-color: var(--accent);
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
    max-height: 320px;
    overflow-y: auto;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  li button {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    width: 100%;
    padding: 0.4rem 0.5rem;
    background: none;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
  }

  li button:hover {
    background: var(--bg-hover);
  }

  li img {
    width: 28px;
    height: 28px;
  }

  .empty {
    padding: 0.5rem;
    color: var(--text-dim);
  }
</style>

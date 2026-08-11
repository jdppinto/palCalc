<script lang="ts">
  // One keyboard-navigable picker for both single-select (a species) and
  // multi-select (passives). Replaces PalSelect + PassivePicker so selection
  // behaves consistently everywhere: ↑/↓ to move, Enter to pick, Esc to close,
  // Backspace to remove the last chip, click a chip's ✕ to clear.
  interface Option {
    key: string;
    name: string;
    icon?: string | null;
    rank?: number;
  }

  let {
    options,
    label,
    multiple = false,
    max = 8,
    showRank = false,
    placeholder = "Type to search…",
    value = $bindable(null),
    selected = $bindable([]),
  }: {
    options: Option[];
    label: string;
    multiple?: boolean;
    max?: number;
    showRank?: boolean;
    placeholder?: string;
    value?: string | null;
    selected?: string[];
  } = $props();

  let query = $state("");
  let open = $state(false);
  let active = $state(0);

  const chosen = $derived(multiple ? selected : value != null ? [value] : []);
  const nameOf = (k: string) => options.find((o) => o.key === k)?.name ?? k;
  const atMax = $derived(multiple && selected.length >= max);
  const showInput = $derived(multiple ? !atMax : chosen.length === 0);

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const avail = options.filter((o) => !chosen.includes(o.key));
    const matched = q ? avail.filter((o) => o.name.toLowerCase().includes(q)) : avail;
    return matched.slice(0, 60);
  });

  // Keep the active index within the current results.
  $effect(() => {
    if (active > filtered.length - 1) active = Math.max(0, filtered.length - 1);
  });

  function choose(key: string) {
    if (multiple) {
      if (selected.length < max && !selected.includes(key)) selected = [...selected, key];
    } else {
      value = key;
      open = false;
    }
    query = "";
    active = 0;
  }
  function removeKey(key: string) {
    if (multiple) selected = selected.filter((k) => k !== key);
    else value = null;
  }
  function onKeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      open = true;
      active = Math.min(active + 1, filtered.length - 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      active = Math.max(active - 1, 0);
    } else if (e.key === "Enter") {
      if (open && filtered[active]) {
        e.preventDefault();
        choose(filtered[active].key);
      }
    } else if (e.key === "Escape") {
      open = false;
    } else if (e.key === "Backspace" && query === "" && chosen.length) {
      removeKey(chosen[chosen.length - 1]);
    }
  }
</script>

<div class="combo">
  <span class="clabel">{label}</span>
  <div class="control">
    {#each chosen as key (key)}
      <button type="button" class="chip" title="Remove" onclick={() => removeKey(key)}>
        {nameOf(key)}<span class="x">✕</span>
      </button>
    {/each}
    {#if showInput}
      <input
        type="text"
        bind:value={query}
        {placeholder}
        onfocus={() => (open = true)}
        onblur={() => setTimeout(() => (open = false), 120)}
        onkeydown={onKeydown}
      />
    {:else if atMax}
      <span class="atmax">max {max}</span>
    {/if}
  </div>

  {#if open && showInput}
    <ul class="menu">
      {#if filtered.length === 0}
        <li class="empty">No matches</li>
      {:else}
        {#each filtered as o, i (o.key)}
          <li>
            <button
              type="button"
              class="opt"
              class:active={i === active}
              onmousedown={() => choose(o.key)}
              onmouseenter={() => (active = i)}
            >
              {#if o.icon}<img src={"/icons/" + o.icon} alt="" />{/if}
              <span class="oname">{o.name}</span>
              {#if showRank && o.rank !== undefined}
                <span class="rank">{o.rank > 0 ? "+" + o.rank : o.rank}</span>
              {/if}
            </button>
          </li>
        {/each}
      {/if}
    </ul>
  {/if}
</div>

<style>
  .combo {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  .control {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.3rem;
    padding: 0.25rem 0.35rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .control:focus-within {
    border-color: var(--accent);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.12rem 0.45rem;
    background: var(--accent-soft);
    border: 1px solid var(--accent);
    border-radius: 999px;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .chip .x {
    color: var(--text-dim);
  }
  .chip:hover .x {
    color: #f85149;
  }
  input {
    flex: 1;
    min-width: 6rem;
    border: none;
    background: none;
    color: var(--text);
    font: inherit;
    padding: 0.15rem;
    outline: none;
  }
  .atmax {
    padding: 0.15rem 0.3rem;
    font-size: 0.75rem;
  }
  .menu {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    z-index: 30;
    margin: 0.2rem 0 0;
    padding: 0.2rem;
    list-style: none;
    max-height: 16rem;
    overflow-y: auto;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
  }
  .empty {
    padding: 0.4rem 0.5rem;
    color: var(--text-dim);
  }
  .opt {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.5rem;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text);
    font: inherit;
    cursor: pointer;
    text-align: left;
  }
  .opt.active {
    background: var(--accent-soft);
  }
  .opt img {
    width: 26px;
    height: 26px;
    object-fit: contain;
  }
  .oname {
    flex: 1;
  }
  .rank {
    color: var(--text-dim);
    font-size: 0.8rem;
  }
</style>

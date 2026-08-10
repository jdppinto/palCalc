<script lang="ts">
  import { bookmarksStore, removeBookmark } from "./bookmarks.svelte";
  import BreedingTree from "./BreedingTree.svelte";

  // Which saved routes have their tree expanded (by bookmark id).
  let expanded = $state<Set<string>>(new Set());
  function toggle(id: string) {
    const s = new Set(expanded);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    expanded = s;
  }

  // saved_at is a Date.now() epoch ms; render it in the local locale.
  function when(ms: number): string {
    try {
      return new Date(ms).toLocaleString();
    } catch {
      return "";
    }
  }
</script>

<section>
  <h2>Bookmarked breedings</h2>
  {#if bookmarksStore.list.length === 0}
    <p class="empty">
      No bookmarks yet. Save a breeding from the Route Planner (☆ Bookmark) or a
      breeding tree (★ Bookmark this tree), and it'll appear here.
    </p>
  {:else}
    <ul>
      {#each bookmarksStore.list as b (b.id)}
        <li>
          <div class="line">
            <div class="info">
              <span class="label">{b.label}</span>
              <span class="meta">saved {when(b.saved_at)}</span>
            </div>
            <button class="open" onclick={() => toggle(b.id)}>
              {expanded.has(b.id) ? "▾ hide tree" : "▸ tree"}
            </button>
            <button class="remove" title="Remove bookmark" onclick={() => removeBookmark(b.id)}>
              Remove
            </button>
          </div>
          {#if expanded.has(b.id)}
            <BreedingTree route={b.route} height="60vh" />
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  section {
    max-width: 900px;
    margin: 0 auto;
    padding: 1.5rem;
  }
  h2 {
    margin: 0 0 1rem;
    font-size: 1.1rem;
  }
  .empty {
    color: var(--text-dim);
    padding: 2rem 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  li {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 0.6rem 0.75rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  .info {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    font-size: 0.72rem;
    color: var(--text-dim);
  }
  .open,
  .remove {
    flex-shrink: 0;
    padding: 0.3rem 0.7rem;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    font-size: 0.82rem;
  }
  .open:hover {
    color: var(--accent);
    border-color: var(--accent);
  }
  .remove:hover {
    color: #d0652a;
    border-color: #d0652a;
  }
</style>

<script lang="ts">
  import { toastStore, dismiss } from "./toast.svelte";
</script>

<div class="toasts" role="region" aria-label="Notifications">
  {#each toastStore.list as t (t.id)}
    <div class="toast {t.kind}" role="status">
      <span class="msg">{t.message}</span>
      {#if t.action}
        <button class="act" onclick={() => { t.action?.run(); dismiss(t.id); }}>
          {t.action.label}
        </button>
      {/if}
      <button class="x" aria-label="Dismiss" onclick={() => dismiss(t.id)}>✕</button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    z-index: 100;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-width: min(90vw, 24rem);
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
    padding: 0.6rem 0.75rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-left: 3px solid var(--text-dim);
    border-radius: 8px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
    font-size: 0.85rem;
    color: var(--text);
  }
  .toast.success {
    border-left-color: var(--success);
  }
  .toast.error {
    border-left-color: var(--danger);
  }
  .toast.info {
    border-left-color: var(--accent);
  }
  .msg {
    flex: 1;
    min-width: 0;
  }
  .act {
    flex-shrink: 0;
    padding: 0.25rem 0.6rem;
    background: var(--accent);
    color: var(--on-accent);
    border: none;
    border-radius: 6px;
    font-size: 0.8rem;
    font-weight: 600;
    cursor: pointer;
  }
  .act:hover {
    filter: brightness(1.08);
  }
  .x {
    flex-shrink: 0;
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    line-height: 1;
    padding: 0;
  }
  .x:hover {
    color: var(--text);
  }
</style>

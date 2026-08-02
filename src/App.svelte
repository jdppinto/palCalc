<script lang="ts">
  import { onMount } from "svelte";
  import BreedingTree from "./lib/BreedingTree.svelte";
  import Calculator from "./lib/Calculator.svelte";
  import { flushSave, initOwnedStore } from "./lib/owned.svelte";
  import PalboxScanner from "./lib/PalboxScanner.svelte";
  import RoutePlanner from "./lib/RoutePlanner.svelte";
  import type { Route } from "./lib/types";

  type Tab = "calculator" | "planner" | "scanner" | "tree";
  let tab = $state<Tab>("calculator");
  let treeRoute = $state<Route | null>(null);

  onMount(() => {
    initOwnedStore();
    window.addEventListener("beforeunload", flushSave);
    return () => window.removeEventListener("beforeunload", flushSave);
  });

  function showTree(route: Route) {
    treeRoute = route;
    tab = "tree";
  }

  const tabs: Array<[Tab, string]> = [
    ["calculator", "Calculator"],
    ["planner", "Route Planner"],
    ["scanner", "Scanner"],
    ["tree", "Tree"],
  ];
</script>

<main>
  <header>
    <h1>PalCalc</h1>
    <nav>
      {#each tabs as [id, name] (id)}
        <button class:active={tab === id} onclick={() => (tab = id)}>
          {name}
        </button>
      {/each}
    </nav>
  </header>

  <!-- Views stay mounted (hidden, not removed) so tab switches never lose state -->
  <div hidden={tab !== "calculator"}><Calculator /></div>
  <div hidden={tab !== "planner"}><RoutePlanner onShowTree={showTree} /></div>
  <div hidden={tab !== "scanner"}><PalboxScanner /></div>
  <div hidden={tab !== "tree"}><BreedingTree route={treeRoute} /></div>
</main>

<style>
  main {
    min-height: 100vh;
  }

  header {
    display: flex;
    align-items: center;
    gap: 2rem;
    padding: 0.75rem 1.5rem;
    background: var(--bg-raised);
    border-bottom: 1px solid var(--border);
  }

  h1 {
    margin: 0;
    font-size: 1.2rem;
    color: var(--accent);
  }

  nav {
    display: flex;
    gap: 0.25rem;
  }

  nav button {
    padding: 0.45rem 0.9rem;
    background: none;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    color: var(--text-dim);
  }

  nav button:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  nav button.active {
    background: var(--accent-soft);
    color: var(--accent);
  }
</style>

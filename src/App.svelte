<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import BreedingTree from "./lib/BreedingTree.svelte";
  import Calculator from "./lib/Calculator.svelte";
  import { flushSave, initOwnedStore } from "./lib/owned.svelte";
  import PalboxScanner from "./lib/PalboxScanner.svelte";
  import RoutePlanner from "./lib/RoutePlanner.svelte";
  import type { Route } from "./lib/types";

  type Tab = "calculator" | "planner" | "scanner" | "tree";
  let tab = $state<Tab>("calculator");
  let treeRoute = $state<Route | null>(null);

  interface AppVersion { version: string; tag: string | null; prerelease: boolean; dev: boolean }
  let ver = $state<AppVersion | null>(null);

  onMount(() => {
    initOwnedStore();
    invoke<AppVersion>("app_version").then((v) => (ver = v)).catch(() => {});
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
    {#if ver}
      <span
        class="version"
        class:prerelease={ver.prerelease || ver.dev}
        title={ver.version}
      >
        {#if ver.prerelease}PRE-RELEASE {ver.version}
        {:else if ver.dev}DEV {ver.version}
        {:else}{ver.version}{/if}
      </span>
    {/if}
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

  .version {
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.03em;
    color: var(--text-dim);
    padding: 0.1rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    white-space: nowrap;
  }
  .version.prerelease {
    color: #fff;
    background: #b4541e;
    border-color: #d0652a;
  }

  nav {
    display: flex;
    gap: 0.25rem;
    margin-left: auto;
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

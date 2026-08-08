<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import BreedingTree from "./lib/BreedingTree.svelte";
  import Bookmarks from "./lib/Bookmarks.svelte";
  import Calculator from "./lib/Calculator.svelte";
  import { flushSave, initOwnedStore } from "./lib/owned.svelte";
  import { flushBookmarks, initBookmarksStore } from "./lib/bookmarks.svelte";
  import PalboxScanner from "./lib/PalboxScanner.svelte";
  import RoutePlanner from "./lib/RoutePlanner.svelte";
  import type { Route } from "./lib/types";

  type Tab = "calculator" | "planner" | "scanner" | "tree" | "bookmarks";
  let tab = $state<Tab>("calculator");
  let treeRoute = $state<Route | null>(null);

  interface AppVersion { version: string; tag: string | null; prerelease: boolean; dev: boolean }
  let ver = $state<AppVersion | null>(null);

  onMount(() => {
    initOwnedStore();
    initBookmarksStore();
    invoke<AppVersion>("app_version").then((v) => (ver = v)).catch(() => {});
    const flush = () => { flushSave(); flushBookmarks(); };
    window.addEventListener("beforeunload", flush);
    return () => window.removeEventListener("beforeunload", flush);
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
    ["bookmarks", "Bookmarks"],
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
    <!-- Only shown for prerelease/dev builds, floated to the far right so it
         never displaces the nav. A clean release shows nothing. -->
    {#if ver && (ver.prerelease || ver.dev)}
      <span class="version prerelease" title={ver.version}>
        {ver.prerelease ? "PRE-RELEASE" : "DEV"} {ver.version}
      </span>
    {/if}
  </header>

  <!-- Views stay mounted (hidden, not removed) so tab switches never lose state -->
  <div hidden={tab !== "calculator"}><Calculator /></div>
  <div hidden={tab !== "planner"}><RoutePlanner onShowTree={showTree} /></div>
  <div hidden={tab !== "scanner"}><PalboxScanner /></div>
  <div hidden={tab !== "tree"}><BreedingTree route={treeRoute} /></div>
  <div hidden={tab !== "bookmarks"}><Bookmarks onOpen={showTree} /></div>
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
    /* Pin the nav so it can't scroll away when a tab's content (e.g. a tall
       breeding tree) overflows the viewport. Opaque background + z-index so
       content scrolls under it. */
    position: sticky;
    top: 0;
    z-index: 20;
  }

  h1 {
    margin: 0;
    font-size: 1.2rem;
    color: var(--accent);
  }

  .version {
    margin-left: auto;
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.03em;
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
    white-space: nowrap;
    color: #fff;
    background: #b4541e;
    border: 1px solid #d0652a;
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

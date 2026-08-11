<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import BreedingTree from "./lib/BreedingTree.svelte";
  import Bookmarks from "./lib/BookmarksTab.svelte";
  import Calculator from "./lib/Calculator.svelte";
  import { flushSave, initOwnedStore, ownedStore } from "./lib/owned.svelte";
  import { flushBookmarks, initBookmarksStore } from "./lib/bookmarks.svelte";
  import RoutePlanner from "./lib/RoutePlanner.svelte";
  import Roster from "./lib/Roster.svelte";
  import Toasts from "./lib/Toasts.svelte";
  import type { Route } from "./lib/types";

  type Tab = "plan" | "roster" | "calculator" | "saved" | "tree";
  let tab = $state<Tab>("plan");
  let treeRoute = $state<Route | null>(null);

  interface AppVersion { version: string; tag: string | null; prerelease: boolean; dev: boolean }
  let ver = $state<AppVersion | null>(null);

  onMount(() => {
    // Empty-aware landing: brand-new users (no owned pals) land on Roster to
    // add some; returning users stay on Plan. Only flips if untouched.
    initOwnedStore().then(() => {
      if (tab === "plan" && ownedStore.list.length === 0) tab = "roster";
    });
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
    ["plan", "Plan"],
    ["roster", "Roster"],
    ["calculator", "Calculator"],
    ["saved", "Saved"],
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
    <!-- Only shown for prerelease/dev builds, floated to the far right so it
         never displaces the nav. A clean release shows nothing. -->
    {#if ver && (ver.prerelease || ver.dev)}
      <span class="version prerelease" title={ver.version}>
        {ver.prerelease ? "PRE-RELEASE" : "DEV"} {ver.version}
      </span>
    {/if}
  </header>

  <!-- Views stay mounted (hidden, not removed) so tab switches never lose state -->
  <div hidden={tab !== "plan"}><RoutePlanner onShowTree={showTree} onManageRoster={() => (tab = "roster")} /></div>
  <div hidden={tab !== "roster"}><Roster /></div>
  <div hidden={tab !== "calculator"}><Calculator /></div>
  <div hidden={tab !== "saved"}><Bookmarks onOpen={showTree} /></div>
  <div hidden={tab !== "tree"}><BreedingTree route={treeRoute} /></div>

  <Toasts />
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
    background: var(--warning);
    border: 1px solid var(--warning);
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

<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import BreedingTree from "./lib/BreedingTree.svelte";
  import Bookmarks from "./lib/BookmarksTab.svelte";
  import Calculator from "./lib/Calculator.svelte";
  import { flushSave, initOwnedStore, ownedStore } from "./lib/owned.svelte";
  import { flushBookmarks, initBookmarksStore, resolveBookmark } from "./lib/bookmarks.svelte";
  import { initServerSync } from "./lib/serverSync.svelte";
  import RoutePlanner from "./lib/RoutePlanner.svelte";
  import Roster from "./lib/Roster.svelte";
  import Toasts from "./lib/Toasts.svelte";
  import { check } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";
  import { toast } from "./lib/toast.svelte";
  import type { Bookmark, BookmarkGoal, Route } from "./lib/types";

  type Tab = "plan" | "roster" | "calculator" | "saved" | "tree";
  let tab = $state<Tab>("plan");
  // What the Tree tab draws: a computed route plus the goal/label it came from
  // (so the tree view can re-bookmark or recompute it).
  let treeRoute = $state<Route | null>(null);
  let treeGoal = $state<BookmarkGoal | null>(null);
  let treeLabel = $state("");

  interface AppVersion { version: string; tag: string | null; prerelease: boolean; dev: boolean }
  let ver = $state<AppVersion | null>(null);

  onMount(() => {
    // Empty-aware landing: brand-new users (no owned pals) land on Roster to
    // add some; returning users stay on Plan. Only flips if untouched.
    initOwnedStore().then(() => {
      if (tab === "plan" && ownedStore.list.length === 0) tab = "roster";
      // Start background server auto-sync once owned pals are loaded, so the
      // first sync doesn't race the persisted list.
      initServerSync();
    });
    initBookmarksStore();
    invoke<AppVersion>("app_version").then((v) => (ver = v)).catch(() => {});
    void checkForUpdate();
    const flush = () => { flushSave(); flushBookmarks(); };
    window.addEventListener("beforeunload", flush);
    return () => window.removeEventListener("beforeunload", flush);
  });

  // Self-update: on launch, ask GitHub for a newer signed release. If one
  // exists, offer a one-click "Update & restart". Errors are swallowed —
  // offline, no release yet, or running unpackaged in dev all land here.
  async function checkForUpdate() {
    try {
      const update = await check();
      if (!update) return;
      toast.action(`PalCalc ${update.version} is available.`, {
        label: "Update & restart",
        run: async () => {
          try {
            toast.info("Downloading update…");
            await update.downloadAndInstall();
            await relaunch();
          } catch (e) {
            toast.error(`Update failed: ${e}`);
          }
        },
      });
    } catch {
      /* offline, no release yet, or unpackaged — ignore */
    }
  }

  function showTree(route: Route, goal: BookmarkGoal | null = null, label = "") {
    treeRoute = route;
    treeGoal = goal;
    treeLabel = label;
    tab = "tree";
  }

  // Opening a bookmark re-plans its goal against the CURRENT roster, so the tree
  // reflects newly-acquired pals. If nothing's reachable now, say so plainly
  // instead of showing a stale tree.
  async function openBookmark(b: Bookmark) {
    try {
      const route = await resolveBookmark(b);
      if (route) {
        showTree(route, b, b.label);
      } else {
        toast.error(`No route to ${b.label} with your current roster yet.`);
      }
    } catch (e) {
      toast.error(`Couldn't plan ${b.label}: ${e}`);
    }
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
  <div hidden={tab !== "saved"}><Bookmarks onOpen={openBookmark} /></div>
  <div hidden={tab !== "tree"}><BreedingTree route={treeRoute} goal={treeGoal} label={treeLabel} /></div>

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

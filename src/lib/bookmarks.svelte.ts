import { invoke } from "@tauri-apps/api/core";
import type { Bookmark, Route, RouteNode } from "./types";
import { toast } from "./toast.svelte";

/// Build a Bookmark from a route — the single source of the label format, so
/// the Route Planner and the Tree view produce identical labels for the same
/// route (which is also what the bookmarked() dedup check compares on).
export function bookmarkLabel(route: Route): string {
  const passives = route.covered.length ? route.covered.join(", ") : "no passives";
  return `${route.root.name} · ${route.steps} step${route.steps === 1 ? "" : "s"} · ${passives}`;
}

/// A stable identity for a route from its tree structure — species, gender,
/// owned marker, and desired passives per node, order-insensitive on parents.
/// Used for dedup/isBookmarked instead of the display label, so a label-format
/// change can't break identity and two structurally-different routes that
/// happen to share a label can both be saved.
function nodeKey(n: RouteNode): string {
  const pv = [...n.passives].sort().join(",");
  const kids = n.parents.map(nodeKey).sort().join("|");
  return `${n.species}/${n.gender ?? ""}/${n.owned ?? ""}/${pv}(${kids})`;
}
export function routeKey(route: Route): string {
  return nodeKey(route.root);
}

export function makeBookmark(route: Route): Bookmark {
  return {
    id: crypto.randomUUID(),
    key: routeKey(route),
    label: bookmarkLabel(route),
    saved_at: Date.now(),
    route,
  };
}

/// Whether this route (by structural key) is already bookmarked.
export function isBookmarked(route: Route): boolean {
  const key = routeKey(route);
  return bookmarksStore.list.some((b) => b.key === key);
}

// Saved breeding-tree bookmarks, persisted to disk via the backend — same
// pattern as the owned-pals store (debounced save, load on startup).

let _initialized = false;
let _saveTimer: ReturnType<typeof setTimeout> | null = null;

export const bookmarksStore = $state<{ list: Bookmark[] }>({ list: [] });

export async function initBookmarksStore() {
  try {
    const list = await invoke<Bookmark[]>("load_bookmarks");
    // Backfill the structural key on bookmarks saved before it existed.
    bookmarksStore.list = Array.isArray(list)
      ? list.map((b) => (b.key ? b : { ...b, key: routeKey(b.route) }))
      : [];
  } catch (e) {
    console.error("Failed to load bookmarks:", e);
    bookmarksStore.list = [];
  }
  _initialized = true;
}

function save() {
  if (!_initialized) return;
  if (_saveTimer) clearTimeout(_saveTimer);
  _saveTimer = setTimeout(() => {
    invoke("save_bookmarks", { bookmarks: bookmarksStore.list }).catch((e) => {
      console.error("Failed to save bookmarks:", e);
      toast.error("Couldn't save bookmarks to disk.");
    });
  }, 100);
}

export function flushBookmarks() {
  if (_saveTimer) {
    clearTimeout(_saveTimer);
    _saveTimer = null;
  }
  if (_initialized) {
    invoke("save_bookmarks", { bookmarks: bookmarksStore.list }).catch((e) => {
      console.error("Failed to save bookmarks:", e);
      toast.error("Couldn't save bookmarks to disk.");
    });
  }
}

/// Add a bookmark, newest first. No-op if one with the same label already
/// exists (the same route bookmarked twice from either view).
export function addBookmark(b: Bookmark) {
  if (bookmarksStore.list.some((x) => x.key === b.key)) return;
  bookmarksStore.list = [b, ...bookmarksStore.list];
  save();
}

export function removeBookmark(id: string) {
  bookmarksStore.list = bookmarksStore.list.filter((b) => b.id !== id);
  save();
}

/// Add the route if not bookmarked, remove it if it is — matched by key.
export function toggleBookmark(route: Route) {
  const key = routeKey(route);
  const existing = bookmarksStore.list.find((b) => b.key === key);
  if (existing) {
    removeBookmark(existing.id);
  } else {
    addBookmark(makeBookmark(route));
  }
}

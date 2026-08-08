import { invoke } from "@tauri-apps/api/core";
import type { Bookmark, Route } from "./types";

/// Build a Bookmark from a route — the single source of the label format, so
/// the Route Planner and the Tree view produce identical labels for the same
/// route (which is also what the bookmarked() dedup check compares on).
export function bookmarkLabel(route: Route): string {
  const passives = route.covered.length ? route.covered.join(", ") : "no passives";
  return `${route.root.name} · ${route.steps} step${route.steps === 1 ? "" : "s"} · ${passives}`;
}

export function makeBookmark(route: Route): Bookmark {
  return {
    id: crypto.randomUUID(),
    label: bookmarkLabel(route),
    saved_at: Date.now(),
    route,
  };
}

/// Whether a route with this label is already bookmarked.
export function isBookmarked(route: Route): boolean {
  const label = bookmarkLabel(route);
  return bookmarksStore.list.some((b) => b.label === label);
}

// Saved breeding-tree bookmarks, persisted to disk via the backend — same
// pattern as the owned-pals store (debounced save, load on startup).

let _initialized = false;
let _saveTimer: ReturnType<typeof setTimeout> | null = null;

export const bookmarksStore = $state<{ list: Bookmark[] }>({ list: [] });

export async function initBookmarksStore() {
  try {
    const list = await invoke<Bookmark[]>("load_bookmarks");
    bookmarksStore.list = Array.isArray(list) ? list : [];
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
    invoke("save_bookmarks", { bookmarks: bookmarksStore.list }).catch((e) =>
      console.error("Failed to save bookmarks:", e),
    );
  }, 100);
}

export function flushBookmarks() {
  if (_saveTimer) {
    clearTimeout(_saveTimer);
    _saveTimer = null;
  }
  if (_initialized) {
    invoke("save_bookmarks", { bookmarks: bookmarksStore.list }).catch((e) =>
      console.error("Failed to save bookmarks:", e),
    );
  }
}

/// Add a bookmark, newest first. No-op if one with the same label already
/// exists (the same route bookmarked twice from either view).
export function addBookmark(b: Bookmark) {
  if (bookmarksStore.list.some((x) => x.label === b.label)) return;
  bookmarksStore.list = [b, ...bookmarksStore.list];
  save();
}

export function removeBookmark(id: string) {
  bookmarksStore.list = bookmarksStore.list.filter((b) => b.id !== id);
  save();
}

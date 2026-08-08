import { invoke } from "@tauri-apps/api/core";
import type { Bookmark } from "./types";

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

/// Add a bookmark, newest first. No-op if an identical one (same label) is
/// already the most recent — a double-click shouldn't duplicate it.
export function addBookmark(b: Bookmark) {
  if (bookmarksStore.list[0]?.label === b.label) return;
  bookmarksStore.list = [b, ...bookmarksStore.list];
  save();
}

export function removeBookmark(id: string) {
  bookmarksStore.list = bookmarksStore.list.filter((b) => b.id !== id);
  save();
}

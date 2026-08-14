import { invoke } from "@tauri-apps/api/core";
import type {
  Bookmark,
  BookmarkGoal,
  PassiveEntry,
  PlanOutcome,
  Route,
  RouteNode,
} from "./types";
import { ownedStore } from "./owned.svelte";
import { toast } from "./toast.svelte";

/// The single source of the label format, so the Planner and the Tree view
/// produce identical labels for the same goal. Roster-dependent facts (step
/// count) are intentionally absent — those are shown on the recomputed tree.
export function goalLabel(targetName: string, passiveNames: string[]): string {
  const passives = passiveNames.length ? passiveNames.join(", ") : "no passives";
  return `${targetName} · ${passives}`;
}

/// Stable identity for a goal: target + sorted desired passives + assume_wild.
/// (reversers/max_steps are search budget, not part of what you're breeding.)
export function goalKey(g: BookmarkGoal): string {
  return `${g.target}|${[...g.desired_passives].sort().join(",")}|${g.assume_wild ? "w" : ""}`;
}

export function makeBookmark(g: BookmarkGoal, label: string): Bookmark {
  return {
    ...g,
    id: crypto.randomUUID(),
    key: goalKey(g),
    label,
    saved_at: Date.now(),
  };
}

/// Whether this goal (by key) is already bookmarked.
export function isBookmarked(g: BookmarkGoal): boolean {
  const key = goalKey(g);
  return bookmarksStore.list.some((b) => b.key === key);
}

// Saved goal bookmarks, persisted to disk via the backend (debounced save,
// load on startup) — same pattern as the owned-pals store.

let _initialized = false;
let _saveTimer: ReturnType<typeof setTimeout> | null = null;

export const bookmarksStore = $state<{ list: Bookmark[] }>({ list: [] });

export async function initBookmarksStore() {
  try {
    const raw = await invoke<unknown[]>("load_bookmarks");
    const list = Array.isArray(raw) ? raw : [];
    // Legacy bookmarks stored a frozen `route` and no `target`. Convert them to
    // goal bookmarks, recovering desired passives (display names → keys) via the
    // passive catalog. Loaded only when a migration is actually needed.
    const needsMigration = list.some(
      (b) => b && typeof b === "object" && "route" in b && !("target" in b),
    );
    let nameToKey: Map<string, string> = new Map();
    if (needsMigration) {
      try {
        const passives = await invoke<PassiveEntry[]>("list_passives");
        nameToKey = new Map(passives.map((p) => [p.name, p.key]));
      } catch (e) {
        console.error("bookmark migration: failed to load passives", e);
      }
    }
    bookmarksStore.list = list
      .map((b) => migrate(b, nameToKey))
      .filter((b): b is Bookmark => b !== null);
    _initialized = true;
    // Persist the migrated shape so legacy route blobs are replaced on disk.
    if (needsMigration) save();
  } catch (e) {
    console.error("Failed to load bookmarks:", e);
    bookmarksStore.list = [];
    _initialized = true;
  }
}

function hasWildLeaf(n: RouteNode): boolean {
  return n.owned === "wild" || n.parents.some(hasWildLeaf);
}

/// Coerce a loaded record — new goal shape or legacy route shape — into a
/// Bookmark, or null if it can't be salvaged.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function migrate(b: any, nameToKey: Map<string, string>): Bookmark | null {
  if (!b || typeof b !== "object") return null;

  // Already goal-shaped: keep as-is (backfill any missing bits defensively).
  if (typeof b.target === "string" && Array.isArray(b.desired_passives)) {
    const g: BookmarkGoal = {
      target: b.target,
      desired_passives: b.desired_passives,
      assume_wild: !!b.assume_wild,
      reversers: b.reversers ?? 0,
      max_steps: b.max_steps ?? 500,
    };
    return {
      ...g,
      id: b.id ?? crypto.randomUUID(),
      key: b.key ?? goalKey(g),
      label: b.label ?? b.target,
      saved_at: b.saved_at ?? Date.now(),
    };
  }

  // Legacy: reconstruct the goal from the stored route.
  const route: Route | undefined = b.route;
  if (!route || !route.root) return null;
  const desiredNames = [...(route.covered ?? []), ...(route.missing ?? [])];
  const desired_passives = desiredNames
    .map((n) => nameToKey.get(n))
    .filter((k): k is string => !!k);
  const g: BookmarkGoal = {
    target: route.root.species,
    desired_passives,
    assume_wild: hasWildLeaf(route.root),
    reversers: route.reversers_used ?? 0,
    max_steps: 500,
  };
  return {
    ...g,
    id: b.id ?? crypto.randomUUID(),
    key: goalKey(g),
    // Rebuild the label from the goal (all desired passives, not just the ones
    // this old route happened to cover) so it matches freshly-saved bookmarks.
    label: goalLabel(route.root.name, desiredNames),
    saved_at: b.saved_at ?? Date.now(),
  };
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

/// Add a bookmark, newest first. No-op if the same goal (by key) already exists.
export function addBookmark(b: Bookmark) {
  if (bookmarksStore.list.some((x) => x.key === b.key)) return;
  bookmarksStore.list = [b, ...bookmarksStore.list];
  save();
}

export function removeBookmark(id: string) {
  bookmarksStore.list = bookmarksStore.list.filter((b) => b.id !== id);
  save();
}

/// Add the goal if not bookmarked, remove it if it is — matched by key.
export function toggleBookmark(g: BookmarkGoal, label: string) {
  const key = goalKey(g);
  const existing = bookmarksStore.list.find((b) => b.key === key);
  if (existing) {
    removeBookmark(existing.id);
  } else {
    addBookmark(makeBookmark(g, label));
  }
}

/// Re-plan a bookmarked goal against the CURRENT roster. Returns the best route,
/// or null if the target is unreachable with the roster as it stands now. This
/// is what makes bookmarks "live" — a newly-acquired pal can shorten the tree.
export async function resolveBookmark(g: BookmarkGoal): Promise<Route | null> {
  const out = await invoke<PlanOutcome>("plan", {
    req: {
      target: g.target,
      desired_passives: g.desired_passives,
      owned: ownedStore.list,
      assume_wild: g.assume_wild,
      max_steps: g.max_steps,
      reversers: g.reversers,
    },
  });
  return out.routes[0] ?? null;
}

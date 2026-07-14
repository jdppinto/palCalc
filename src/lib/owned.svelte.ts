// Shared owned-pals store: the Route Planner edits it, the Scanner feeds it.
// Persisted to localStorage; loaded synchronously so a save can never observe
// (and clobber) a transient empty list.
import type { OwnedPal } from "./types";

const KEY = "palcalc.owned";

function load(): OwnedPal[] {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "[]");
  } catch {
    return [];
  }
}

export const ownedStore = $state<{ list: OwnedPal[] }>({ list: load() });

function save() {
  localStorage.setItem(KEY, JSON.stringify(ownedStore.list));
}

export function addOwnedPal(p: OwnedPal) {
  ownedStore.list = [...ownedStore.list, p];
  save();
}

export function addManyOwned(ps: OwnedPal[]) {
  ownedStore.list = [...ownedStore.list, ...ps];
  save();
}

export function removeOwnedAt(i: number) {
  ownedStore.list = ownedStore.list.filter((_, j) => j !== i);
  save();
}

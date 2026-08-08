import { invoke } from "@tauri-apps/api/core";
import type { OwnedPal } from "./types";

let _initialized = false;
let _saveTimer: ReturnType<typeof setTimeout> | null = null;

export const ownedStore = $state<{ list: OwnedPal[] }>({ list: [] });

async function migrateFromLocalStorage() {
  const raw = localStorage.getItem("palcalc.owned");
  if (raw) {
    try {
      const migrated = JSON.parse(raw) as OwnedPal[];
      ownedStore.list = migrated;
      await invoke("save_owned_pals", { pals: migrated }).catch(() => {});
      localStorage.removeItem("palcalc.owned");
    } catch (e) {
      console.error("Failed to migrate owned pals from localStorage:", e);
    }
  }
}

export async function initOwnedStore() {
  try {
    const pals = await invoke<OwnedPal[]>("load_owned_pals");
    if (pals.length > 0) {
      ownedStore.list = pals;
    } else {
      await migrateFromLocalStorage();
    }
  } catch {
    await migrateFromLocalStorage();
  }
  _initialized = true;
}

function save() {
  if (!_initialized) return;
  if (_saveTimer) clearTimeout(_saveTimer);
  _saveTimer = setTimeout(() => {
    invoke("save_owned_pals", { pals: ownedStore.list }).catch((e) =>
      console.error("Failed to save owned pals:", e),
    );
  }, 100);
}

export function flushSave() {
  if (_saveTimer) {
    clearTimeout(_saveTimer);
    _saveTimer = null;
  }
  if (_initialized) {
    invoke("save_owned_pals", { pals: ownedStore.list }).catch((e) =>
      console.error("Failed to save owned pals:", e),
    );
  }
}

export function addOwnedPal(p: OwnedPal) {
  ownedStore.list = [...ownedStore.list, p];
  save();
}

export function addManyOwned(ps: OwnedPal[]) {
  ownedStore.list = [...ownedStore.list, ...ps];
  save();
}

/// Replace the whole list — a full-box sweep is a complete inventory
/// snapshot, not an increment.
export function replaceAllOwned(ps: OwnedPal[]) {
  ownedStore.list = ps;
  save();
}

export function removeOwnedAt(i: number) {
  ownedStore.list = ownedStore.list.filter((_, j) => j !== i);
  save();
}

export function clearAllOwned() {
  ownedStore.list = [];
  save();
}

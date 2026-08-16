import { invoke } from "@tauri-apps/api/core";
import type { OwnedPal } from "./types";
import { toast } from "./toast.svelte";

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
    invoke("save_owned_pals", { pals: ownedStore.list }).catch((e) => {
      console.error("Failed to save owned pals:", e);
      toast.error("Couldn't save your pals to disk.");
    });
  }, 100);
}

export function flushSave() {
  if (_saveTimer) {
    clearTimeout(_saveTimer);
    _saveTimer = null;
  }
  if (_initialized) {
    invoke("save_owned_pals", { pals: ownedStore.list }).catch((e) => {
      console.error("Failed to save owned pals:", e);
      toast.error("Couldn't save your pals to disk.");
    });
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

export function removeOwnedAt(i: number) {
  ownedStore.list = ownedStore.list.filter((_, j) => j !== i);
  save();
}

export function clearAllOwned() {
  ownedStore.list = [];
  save();
}

/// Replace all server-imported pals with a fresh set, leaving manually-added
/// and scanned pals untouched. Used by server auto-sync so background polling
/// refreshes your palbox without clobbering pals you added by hand or scanned.
export function replaceServerOwned(serverPals: OwnedPal[]) {
  const kept = ownedStore.list.filter((p) => p.source !== "server");
  ownedStore.list = [...kept, ...serverPals];
  save();
}

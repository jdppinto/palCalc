import { invoke } from "@tauri-apps/api/core";
import type { OwnedPal } from "./types";

let _initialized = false;
let _pendingSave: OwnedPal[] | null = null;
let _saveTimer: ReturnType<typeof setTimeout> | null = null;

export const ownedStore = $state<{ list: OwnedPal[] }>({ list: [] });

export async function initOwnedStore() {
  try {
    const pals = await invoke<OwnedPal[]>("load_owned_pals");
    if (pals.length > 0) {
      ownedStore.list = pals;
    } else {
      const raw = localStorage.getItem("palcalc.owned");
      if (raw) {
        const migrated = JSON.parse(raw) as OwnedPal[];
        ownedStore.list = migrated;
        await invoke("save_owned_pals", { pals: migrated });
        localStorage.removeItem("palcalc.owned");
      }
    }
  } catch {
    const raw = localStorage.getItem("palcalc.owned");
    if (raw) {
      const migrated = JSON.parse(raw) as OwnedPal[];
      ownedStore.list = migrated;
      await invoke("save_owned_pals", { pals: migrated });
      localStorage.removeItem("palcalc.owned");
    }
  }
  _initialized = true;
  if (_pendingSave) {
    invoke("save_owned_pals", { pals: _pendingSave });
    _pendingSave = null;
  }
}

function save() {
  if (!_initialized) {
    _pendingSave = [...ownedStore.list];
    return;
  }
  if (_saveTimer) clearTimeout(_saveTimer);
  _saveTimer = setTimeout(() => {
    invoke("save_owned_pals", { pals: ownedStore.list });
  }, 100);
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

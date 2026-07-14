<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { addManyOwned } from "./owned.svelte";
  import type { PalEntry } from "./types";

  interface Calib {
    slot_tl: [number, number];
    slot_br: [number, number];
    cols: number;
    rows: number;
    slot_size: number;
    delay_ms: number;
  }

  interface ScannerStatus {
    backend: string | null;
    error: string | null;
    calibration: Calib | null;
  }

  interface SlotResult {
    row: number;
    col: number;
    species: string | null;
    score: number;
  }

  let pals = $state<PalEntry[]>([]);
  let status = $state<ScannerStatus | null>(null);
  let calib = $state<Calib>({
    slot_tl: [0, 0],
    slot_br: [0, 0],
    cols: 6,
    rows: 5,
    slot_size: 90,
    delay_ms: 300,
  });
  let calibSaved = $state(false);
  let countdown = $state(0);
  let capturing = $state<0 | 1 | 2>(0);
  let scanning = $state(false);
  let progress = $state<{ current: number; total: number } | null>(null);
  let results = $state<SlotResult[] | null>(null);
  let error = $state<string | null>(null);

  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  onMount(async () => {
    pals = await invoke<PalEntry[]>("list_pals");
    status = await invoke<ScannerStatus>("scanner_status");
    if (status.calibration) {
      calib = status.calibration;
      calibSaved = true;
    }
    await listen<{ current: number; total: number }>("scan-progress", (e) => {
      progress = e.payload;
    });
  });

  function pal(key: string | null): PalEntry | undefined {
    return pals.find((p) => p.key === key);
  }

  async function captureCorner(which: 1 | 2) {
    capturing = which;
    for (let i = 5; i > 0; i--) {
      countdown = i;
      await sleep(1000);
    }
    countdown = 0;
    try {
      const [x, y] = await invoke<[number, number]>("get_cursor_pos");
      if (which === 1) calib.slot_tl = [x, y];
      else calib.slot_br = [x, y];
      calibSaved = false;
    } catch (e) {
      error = String(e);
    }
    capturing = 0;
  }

  async function saveCalib() {
    try {
      await invoke("save_calibration", { calib });
      calibSaved = true;
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  async function scan() {
    scanning = true;
    error = null;
    results = null;
    progress = null;
    try {
      results = await invoke<SlotResult[]>("scan_current_box", {});
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
  }

  const found = $derived(results?.filter((r) => r.species !== null) ?? []);

  function addAll() {
    addManyOwned(
      found.map((r) => ({
        species: r.species!,
        label: `${pal(r.species)?.name ?? r.species} (scan)`,
        passives: [],
      })),
    );
    results = null;
  }
</script>

<section>
  {#if status?.error}
    <p class="banner error">{status.error}</p>
  {:else if status?.backend}
    <p class="banner ok">capture backend: {status.backend}</p>
  {/if}

  <details open={!calibSaved}>
    <summary>Calibration {calibSaved ? "✓" : "(needed before scanning)"}</summary>
    <ol>
      <li>Open the Palbox in Palworld.</li>
      <li>
        Click a capture button below, then place your mouse over the CENTER of
        that slot before the countdown ends. Don't click anything in the game —
        the scanner only ever hovers.
      </li>
    </ol>
    <div class="row">
      <button onclick={() => captureCorner(1)} disabled={capturing !== 0}>
        {capturing === 1 && countdown ? `…${countdown}` : "Capture top-left slot"}
      </button>
      <span class="pos">({calib.slot_tl[0]}, {calib.slot_tl[1]})</span>
      <button onclick={() => captureCorner(2)} disabled={capturing !== 0}>
        {capturing === 2 && countdown ? `…${countdown}` : "Capture bottom-right slot"}
      </button>
      <span class="pos">({calib.slot_br[0]}, {calib.slot_br[1]})</span>
    </div>
    <div class="row">
      <label>cols <input type="number" min="1" bind:value={calib.cols} /></label>
      <label>rows <input type="number" min="1" bind:value={calib.rows} /></label>
      <label>slot px <input type="number" min="20" bind:value={calib.slot_size} /></label>
      <label>
        delay {calib.delay_ms} ms
        <input type="range" min="100" max="1000" step="50" bind:value={calib.delay_ms} />
      </label>
      <button class="save" onclick={saveCalib}>Save calibration</button>
    </div>
  </details>

  <div class="row scan-row">
    <button class="scan" onclick={scan} disabled={scanning || !calibSaved || !status?.backend}>
      {scanning ? "Scanning…" : "Scan current box"}
    </button>
    {#if scanning}
      <button onclick={() => invoke("abort_scan")}>Abort</button>
      {#if progress}
        <progress value={progress.current} max={progress.total}></progress>
        <span class="pos">{progress.current} / {progress.total}</span>
      {/if}
    {/if}
    <span class="hint-inline">
      Scan one box, switch to the next box in-game, scan again — results
      accumulate in Owned Pals.
    </span>
  </div>

  {#if error}
    <p class="banner error">{error}</p>
  {/if}

  {#if results}
    <div class="results">
      {#each results as r (r.row * 100 + r.col)}
        <div class="slot" class:empty={!r.species}>
          {#if r.species}
            {@const p = pal(r.species)}
            {#if p?.icon}<img src={"/icons/" + p.icon} alt="" />{/if}
            <span>{p?.name ?? r.species}</span>
            <span class="score">{r.score.toFixed(2)}</span>
          {:else}
            <span class="dim">empty</span>
          {/if}
        </div>
      {/each}
    </div>
    {#if found.length > 0}
      <button class="add-all" onclick={addAll}>
        Add {found.length} pal{found.length === 1 ? "" : "s"} to Owned Pals
      </button>
      <p class="note">
        Passives aren't read automatically yet — add them per pal in the Route
        Planner's owned list if you need passive-aware routes.
      </p>
    {/if}
  {/if}
</section>

<style>
  section {
    max-width: 860px;
    margin: 0 auto;
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .banner {
    padding: 0.6rem 1rem;
    border-radius: 8px;
    margin: 0;
  }

  .banner.error {
    background: rgba(239, 68, 68, 0.12);
    color: #f87171;
  }

  .banner.ok {
    background: rgba(34, 197, 94, 0.1);
    color: #4ade80;
  }

  details {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.6rem 0.9rem;
  }

  summary {
    cursor: pointer;
    color: var(--text-dim);
  }

  ol {
    color: var(--text-dim);
    font-size: 0.9rem;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
    margin-top: 0.75rem;
  }

  button {
    padding: 0.5rem 0.9rem;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .pos {
    color: var(--text-dim);
    font-size: 0.85rem;
  }

  label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--text-dim);
    font-size: 0.9rem;
  }

  label input[type="number"] {
    width: 4rem;
    padding: 0.3rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
  }

  .save,
  .scan {
    background: var(--accent);
    color: #1a1408;
    font-weight: 600;
    border: none;
  }

  .hint-inline {
    color: var(--text-dim);
    font-size: 0.85rem;
  }

  .results {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 0.5rem;
  }

  .slot {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .slot.empty {
    opacity: 0.45;
  }

  .slot img {
    width: 28px;
    height: 28px;
  }

  .score {
    margin-left: auto;
    color: var(--text-dim);
    font-size: 0.75rem;
  }

  .dim {
    color: var(--text-dim);
  }

  .add-all {
    align-self: flex-start;
    background: var(--accent);
    color: #1a1408;
    font-weight: 600;
    border: none;
  }

  .note {
    margin: 0;
    color: var(--text-dim);
    font-size: 0.85rem;
  }
</style>

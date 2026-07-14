<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { addManyOwned } from "./owned.svelte";
  import type { Gender, PalEntry, PassiveEntry } from "./types";

  interface Calib {
    slot_tl: [number, number];
    slot_br: [number, number];
    cols: number;
    rows: number;
    slot_size: number;
    delay_ms: number;
    panel: [number, number, number, number] | null;
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
    unidentified: boolean;
    score: number;
    gender: Gender | null;
    passives: string[];
    crop_png: string;
  }

  let pals = $state<PalEntry[]>([]);
  let passiveList = $state<PassiveEntry[]>([]);
  let status = $state<ScannerStatus | null>(null);
  let calib = $state<Calib>({
    slot_tl: [0, 0],
    slot_br: [0, 0],
    cols: 6,
    rows: 5,
    slot_size: 90,
    delay_ms: 300,
    panel: null,
  });
  // Panel corner staging (top-left captured, waiting for bottom-right)
  let panelTl = $state<[number, number] | null>(null);
  let calibSaved = $state(false);
  let countdown = $state(0);
  let capturing = $state<0 | 1 | 2 | 3 | 4>(0);
  let scanning = $state(false);
  let progress = $state<{ current: number; total: number } | null>(null);
  let results = $state<SlotResult[] | null>(null);
  let error = $state<string | null>(null);

  // Species correction: teaches the matcher this game's own rendering
  let fixing = $state<number | null>(null);
  let fixQuery = $state("");

  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  onMount(async () => {
    pals = await invoke<PalEntry[]>("list_pals");
    passiveList = await invoke<PassiveEntry[]>("list_passives");
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

  function passiveName(key: string): string {
    return passiveList.find((p) => p.key === key)?.name ?? key;
  }

  async function useDefaults() {
    try {
      calib = await invoke<Calib>("apply_default_calibration");
      calibSaved = true;
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  async function captureCorner(which: 1 | 2 | 3 | 4) {
    capturing = which;
    for (let i = 5; i > 0; i--) {
      countdown = i;
      await sleep(1000);
    }
    countdown = 0;
    try {
      const [x, y] = await invoke<[number, number]>("get_cursor_pos");
      if (which === 1) calib.slot_tl = [x, y];
      else if (which === 2) calib.slot_br = [x, y];
      else if (which === 3) panelTl = [x, y];
      else if (panelTl) {
        calib.panel = [
          Math.min(panelTl[0], x),
          Math.min(panelTl[1], y),
          Math.abs(x - panelTl[0]),
          Math.abs(y - panelTl[1]),
        ];
      }
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

  const fixMatches = $derived(
    pals
      .filter((p) => p.name.toLowerCase().includes(fixQuery.trim().toLowerCase()))
      .slice(0, 8),
  );

  async function fixSpecies(index: number, speciesKey: string) {
    const r = results?.[index];
    if (!r) return;
    try {
      await invoke("save_pal_template", {
        pngBase64Data: r.crop_png,
        species: speciesKey,
      });
      results = results!.map((s, i) =>
        i === index ? { ...s, species: speciesKey, score: 1.0 } : s,
      );
      fixing = null;
      fixQuery = "";
    } catch (e) {
      error = String(e);
    }
  }

  function genderSymbol(g: Gender | null): string {
    return g === "Male" ? "♂" : g === "Female" ? "♀" : "";
  }

  function addAll() {
    addManyOwned(
      found.map((r) => ({
        species: r.species!,
        label: `${pal(r.species)?.name ?? r.species} (scan)${genderSymbol(r.gender) ? " " + genderSymbol(r.gender) : ""}`,
        passives: r.passives,
      })),
    );
    results = null;
  }
</script>

<section>
  {#if status?.error}
    <p class="banner error">{status.error}</p>
  {:else if status?.backend}
    <p class="banner ok">
      capture backend: {status.backend} · pal names & passives are read from
      the hover panel automatically — no zones to set up
    </p>
  {/if}

  <details open={!calibSaved}>
    <summary>Grid calibration {calibSaved ? "✓" : "(needed before scanning)"}</summary>
    <div class="row">
      <button class="save" onclick={useDefaults}>
        Use default layout (16:9, scaled to your monitor)
      </button>
      <span class="hint-inline">— or calibrate manually below</span>
    </div>
    <ol>
      <li>Open the Palbox in Palworld.</li>
      <li>
        Click a capture button, then place your mouse over the CENTER of that
        slot before the countdown ends. The scanner only ever hovers — never
        click pals in the box.
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
    <h4>Pal sheet (hover panel) bounds</h4>
    <p class="dim-text">
      Hover any pal so its info sheet shows, then capture its top-left and
      bottom-right corners. All name/passive reading happens ONLY inside this
      rectangle.
    </p>
    <div class="row">
      <button onclick={() => captureCorner(3)} disabled={capturing !== 0}>
        {capturing === 3 && countdown ? `…${countdown}` : "Capture sheet top-left"}
      </button>
      <button onclick={() => captureCorner(4)} disabled={capturing !== 0}>
        {capturing === 4 && countdown ? `…${countdown}` : "Capture sheet bottom-right"}
      </button>
      <span class="pos">
        {calib.panel ? `(${calib.panel.join(", ")})` : panelTl ? "top-left set…" : "not set"}
      </span>
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
      Scan one box, switch box in-game, scan again — results accumulate in
      Owned Pals.
    </span>
  </div>

  {#if error}
    <p class="banner error">{error}</p>
  {/if}

  {#if results}
    <p class="dim-text">
      Wrong or unknown species? Click ✎ and pick the right pal — the app
      learns your game's rendering and matches it exactly from then on.
    </p>
    <div class="results">
      {#each results as r, i (r.row * 100 + r.col)}
        <div class="slot" class:empty={!r.species && !r.unidentified}>
          {#if fixing === i}
            <img class="crop" src={"data:image/png;base64," + r.crop_png} alt="slot" />
            <div class="fix-box">
              <input placeholder="Correct pal…" bind:value={fixQuery} />
              <div class="fix-options">
                {#each fixMatches as m (m.key)}
                  <button class="pick" onclick={() => fixSpecies(i, m.key)}>{m.name}</button>
                {/each}
              </div>
            </div>
            <button class="fix" onclick={() => (fixing = null)}>✕</button>
          {:else}
            {#if r.species}
              {@const p = pal(r.species)}
              {#if p?.icon}<img src={"/icons/" + p.icon} alt="" />{/if}
              <div class="slot-info">
                <span>{p?.name ?? r.species} {genderSymbol(r.gender)}</span>
                <span class="passives">
                  {r.passives.map(passiveName).join(", ") || "no passives read"}
                </span>
              </div>
              <span class="score">{r.score.toFixed(2)}</span>
            {:else if r.unidentified}
              <img class="crop" src={"data:image/png;base64," + r.crop_png} alt="unknown pal" />
              <span class="dim">unknown pal — ✎ to teach</span>
            {:else}
              <span class="dim">empty</span>
            {/if}
            <button class="fix" title="Correct species" onclick={() => { fixing = i; fixQuery = ""; }}>
              ✎
            </button>
          {/if}
        </div>
      {/each}
    </div>
    {#if found.length > 0}
      <button class="add-all" onclick={addAll}>
        Add {found.length} pal{found.length === 1 ? "" : "s"} to Owned Pals
      </button>
    {/if}
  {/if}
</section>

<style>
  section {
    max-width: 900px;
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
    white-space: pre-wrap;
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

  ol,
  .dim-text {
    color: var(--text-dim);
    font-size: 0.9rem;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
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
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
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
    width: 30px;
    height: 30px;
  }

  .slot .crop {
    width: 44px;
    height: 44px;
  }

  .slot-info {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .passives {
    font-size: 0.75rem;
    color: var(--text-dim);
  }

  .score {
    margin-left: auto;
    color: var(--text-dim);
    font-size: 0.75rem;
  }

  .fix {
    padding: 0.15rem 0.4rem;
    font-size: 0.8rem;
    background: none;
    border: none;
    color: var(--text-dim);
  }

  .fix:hover {
    color: var(--accent);
  }

  .fix-box {
    flex: 1;
    min-width: 0;
  }

  .fix-box input {
    width: 100%;
    padding: 0.3rem 0.5rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
  }

  .fix-options {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    margin-top: 0.25rem;
  }

  .pick {
    padding: 0.25rem 0.6rem;
    font-size: 0.85rem;
  }

  .add-all {
    align-self: flex-start;
    background: var(--accent);
    color: #1a1408;
    font-weight: 600;
    border: none;
  }
</style>

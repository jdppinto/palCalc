<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { addManyOwned } from "./owned.svelte";
  import type { Gender, PalEntry, PassiveEntry } from "./types";

  type Rect = [number, number, number, number];

  interface Calib {
    slot_tl: [number, number];
    slot_br: [number, number];
    cols: number;
    rows: number;
    slot_size: number;
    delay_ms: number;
    zones: Record<string, Rect>;
  }

  interface ScannerStatus {
    backend: string | null;
    error: string | null;
    calibration: Calib | null;
  }

  type PassiveRead =
    | { kind: "known"; key: string }
    | { kind: "unknown"; id: string; png_base64: string };

  interface SlotResult {
    row: number;
    col: number;
    species: string | null;
    score: number;
    gender: Gender | null;
    passives: PassiveRead[];
    crop_png: string;
  }

  interface FrozenFrame {
    data_url: string;
    x: number;
    y: number;
    w: number;
    h: number;
  }

  const ZONE_KINDS: Array<[string, string]> = [
    ["gender", "Gender"],
    ["passive1", "Passive 1"],
    ["passive2", "Passive 2"],
    ["passive3", "Passive 3"],
    ["passive4", "Passive 4"],
    ["name", "Name (stored, not read yet)"],
  ];

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
    zones: {},
  });
  let calibSaved = $state(false);
  let countdown = $state(0);
  let capturing = $state<0 | 1 | 2 | 3>(0); // 3 = freezing frame
  let scanning = $state(false);
  let progress = $state<{ current: number; total: number } | null>(null);
  let results = $state<SlotResult[] | null>(null);
  let error = $state<string | null>(null);

  // Zone editor state
  let frame = $state<FrozenFrame | null>(null);
  let zoneKind = $state("gender");
  let editorImg = $state<HTMLImageElement | undefined>();
  let drag = $state<{ x0: number; y0: number; x1: number; y1: number } | null>(null);

  // Label queue for unknown passive crops
  let labelQuery = $state("");
  let labeling = $state<string | null>(null); // unknown id being labeled

  // Species correction: teaches the matcher this game's own rendering
  let fixing = $state<number | null>(null); // index into results
  let fixQuery = $state("");

  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  onMount(async () => {
    pals = await invoke<PalEntry[]>("list_pals");
    passiveList = await invoke<PassiveEntry[]>("list_passives");
    status = await invoke<ScannerStatus>("scanner_status");
    if (status.calibration) {
      calib = { zones: {}, ...status.calibration };
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

  async function freezeFrame() {
    capturing = 3;
    error = null;
    for (let i = 5; i > 0; i--) {
      countdown = i;
      await sleep(1000);
    }
    countdown = 0;
    try {
      frame = await invoke<FrozenFrame>("capture_screen");
    } catch (e) {
      error = String(e);
    }
    capturing = 0;
  }

  // Display px → screen px scale for the editor image
  const scale = $derived(
    frame && editorImg ? editorImg.clientWidth / frame.w : 1,
  );

  function editorPos(ev: PointerEvent): { x: number; y: number } {
    const r = editorImg!.getBoundingClientRect();
    return { x: ev.clientX - r.left, y: ev.clientY - r.top };
  }

  function dragStart(ev: PointerEvent) {
    if (!frame || !editorImg) return;
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
    const p = editorPos(ev);
    drag = { x0: p.x, y0: p.y, x1: p.x, y1: p.y };
  }

  function dragMove(ev: PointerEvent) {
    if (!drag) return;
    const p = editorPos(ev);
    drag = { ...drag, x1: p.x, y1: p.y };
  }

  function dragEnd() {
    if (!drag || !frame) return;
    const x = Math.min(drag.x0, drag.x1) / scale + frame.x;
    const y = Math.min(drag.y0, drag.y1) / scale + frame.y;
    const w = Math.abs(drag.x1 - drag.x0) / scale;
    const h = Math.abs(drag.y1 - drag.y0) / scale;
    if (w > 4 && h > 4) {
      calib.zones = {
        ...calib.zones,
        [zoneKind]: [Math.round(x), Math.round(y), Math.round(w), Math.round(h)],
      };
      calibSaved = false;
    }
    drag = null;
  }

  function zoneStyle(r: Rect): string {
    if (!frame) return "";
    return `left:${(r[0] - frame.x) * scale}px;top:${(r[1] - frame.y) * scale}px;width:${r[2] * scale}px;height:${r[3] * scale}px`;
  }

  function removeZone(kind: string) {
    const z = { ...calib.zones };
    delete z[kind];
    calib.zones = z;
    calibSaved = false;
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

  // Unique unknown passive crops across the scan, by id
  const unknowns = $derived.by(() => {
    const seen = new Map<string, string>();
    for (const r of results ?? []) {
      for (const p of r.passives) {
        if (p.kind === "unknown" && !seen.has(p.id)) {
          seen.set(p.id, p.png_base64);
        }
      }
    }
    return [...seen.entries()].map(([id, png]) => ({ id, png }));
  });

  const labelMatches = $derived(
    passiveList
      .filter((p) => p.name.toLowerCase().includes(labelQuery.trim().toLowerCase()))
      .slice(0, 8),
  );

  async function labelUnknown(id: string, png: string, passiveKey: string) {
    try {
      await invoke("save_passive_label", {
        pngBase64Data: png,
        passiveKey,
      });
      // Resolve this id everywhere in the current results
      results =
        results?.map((r) => ({
          ...r,
          passives: r.passives.map((p) =>
            p.kind === "unknown" && p.id === id
              ? { kind: "known" as const, key: passiveKey }
              : p,
          ),
        })) ?? null;
      labeling = null;
      labelQuery = "";
    } catch (e) {
      error = String(e);
    }
  }

  function genderSymbol(g: Gender | null): string {
    return g === "Male" ? "♂" : g === "Female" ? "♀" : "";
  }

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

  function addAll() {
    addManyOwned(
      found.map((r) => ({
        species: r.species!,
        label: `${pal(r.species)?.name ?? r.species} (scan)${genderSymbol(r.gender) ? " " + genderSymbol(r.gender) : ""}`,
        passives: r.passives.flatMap((p) =>
          p.kind === "known" && p.key !== "__empty__" ? [p.key] : [],
        ),
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
        Grid: click a capture button, then place your mouse over the CENTER of
        that slot before the countdown ends. The scanner only ever hovers —
        never click pals in the box.
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
    </div>

    <h4>Hover-panel zones</h4>
    <p class="dim-text">
      Hover any pal in-game so its info panel shows, then freeze the frame and
      drag a rectangle for each field. The panel sits at a fixed position, so
      one calibration covers every slot.
    </p>
    <div class="row">
      <button onclick={freezeFrame} disabled={capturing !== 0}>
        {capturing === 3 && countdown ? `…${countdown}` : "Freeze frame (5s)"}
      </button>
      {#each ZONE_KINDS as [kind, label] (kind)}
        <button
          class="zone-pick"
          class:active={zoneKind === kind}
          class:defined={calib.zones[kind] !== undefined}
          onclick={() => (zoneKind = kind)}
        >
          {label}{calib.zones[kind] ? " ✓" : ""}
        </button>
      {/each}
    </div>
    {#if frame}
      <div
        class="editor"
        role="application"
        onpointerdown={dragStart}
        onpointermove={dragMove}
        onpointerup={dragEnd}
      >
        <img bind:this={editorImg} src={frame.data_url} alt="frozen screen" draggable="false" />
        {#each Object.entries(calib.zones) as [kind, r] (kind)}
          <div class="zone" style={zoneStyle(r)}>
            <span>{kind}</span>
            <button class="zone-x" onpointerdown={(e) => e.stopPropagation()} onclick={() => removeZone(kind)}>✕</button>
          </div>
        {/each}
        {#if drag}
          <div
            class="zone dragging"
            style={`left:${Math.min(drag.x0, drag.x1)}px;top:${Math.min(drag.y0, drag.y1)}px;width:${Math.abs(drag.x1 - drag.x0)}px;height:${Math.abs(drag.y1 - drag.y0)}px`}
          ></div>
        {/if}
      </div>
      <p class="dim-text">Drawing zone: <strong>{zoneKind}</strong> — drag on the image.</p>
    {/if}
    <div class="row">
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

  {#if unknowns.length > 0}
    <div class="label-panel">
      <h4>Unknown passives — label once, matched forever</h4>
      {#each unknowns as u (u.id)}
        <div class="unknown">
          <img src={"data:image/png;base64," + u.png} alt="unknown passive" />
          {#if labeling === u.id}
            <input
              placeholder="Type passive name…"
              bind:value={labelQuery}
            />
            {#each labelMatches as m (m.key)}
              <button class="pick" onclick={() => labelUnknown(u.id, u.png, m.key)}>
                {m.name}
              </button>
            {/each}
            <button class="pick empty-pick" onclick={() => labelUnknown(u.id, u.png, "__empty__")}>
              Empty row
            </button>
          {:else}
            <button onclick={() => (labeling = u.id)}>Label…</button>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if results}
    <p class="dim-text">
      Wrong species? Click ✎ and pick the right pal — the app learns your
      game's rendering and matches it exactly from then on.
    </p>
    <div class="results">
      {#each results as r, i (r.row * 100 + r.col)}
        <div class="slot" class:empty={!r.species}>
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
                  {r.passives
                    .filter((pr) => !(pr.kind === "known" && pr.key === "__empty__"))
                    .map((pr) => (pr.kind === "known" ? passiveName(pr.key) : "?"))
                    .join(", ") || "no passives read"}
                </span>
              </div>
              <span class="score">{r.score.toFixed(2)}</span>
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

  h4 {
    margin: 1rem 0 0.25rem;
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

  .zone-pick.active {
    border-color: var(--accent);
    color: var(--accent);
  }

  .zone-pick.defined {
    background: rgba(34, 197, 94, 0.12);
  }

  .editor {
    position: relative;
    margin-top: 0.75rem;
    user-select: none;
    touch-action: none;
    cursor: crosshair;
    max-height: 60vh;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .editor img {
    width: 100%;
    display: block;
    pointer-events: none;
  }

  .zone {
    position: absolute;
    border: 2px solid var(--accent);
    background: rgba(245, 158, 11, 0.15);
    font-size: 0.7rem;
    color: var(--accent);
  }

  .zone.dragging {
    border-style: dashed;
  }

  .zone span {
    position: absolute;
    top: -1.2rem;
    left: 0;
    background: var(--bg);
    padding: 0 0.3rem;
    border-radius: 4px;
  }

  .zone-x {
    position: absolute;
    top: -1.3rem;
    right: 0;
    padding: 0 0.3rem;
    font-size: 0.7rem;
    background: var(--bg);
    border: none;
    color: #f87171;
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

  .label-panel {
    border: 1px solid var(--accent);
    border-radius: 8px;
    padding: 0.75rem 1rem;
  }

  .label-panel h4 {
    margin: 0 0 0.5rem;
  }

  .unknown {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
    padding: 0.35rem 0;
  }

  .unknown img {
    background: #000;
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  .unknown input {
    padding: 0.35rem 0.6rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
  }

  .pick {
    padding: 0.25rem 0.6rem;
    font-size: 0.85rem;
  }

  .empty-pick {
    color: var(--text-dim);
    border-style: dashed;
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

  .slot .crop {
    width: 44px;
    height: 44px;
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
</style>

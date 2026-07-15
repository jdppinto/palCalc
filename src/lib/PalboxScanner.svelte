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
    panel: Rect | null;
    // Zone overrides stored as FRACTIONS of the panel (fx,fy,fw,fh), so they
    // track the sheet. Resolve to absolute screen px via zoneAbs().
    zones: Record<string, Rect>;
  }

  interface FrozenFrame {
    data_url: string;
    x: number;
    y: number;
    w: number;
    h: number;
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
    passive_unknowns: [string, string][];
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
    zones: {},
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

  // Unknown passive-row labeling (label once, exact matches forever)
  let labelQuery = $state("");
  let labeling = $state<string | null>(null);

  const unknownRows = $derived.by(() => {
    const seen = new Map<string, string>();
    for (const r of results ?? []) {
      for (const [id, png] of r.passive_unknowns) {
        if (!seen.has(id)) seen.set(id, png);
      }
    }
    return [...seen.entries()].map(([id, png]) => ({ id, png }));
  });

  const labelMatches = $derived(
    passiveList
      .filter((p) => p.name.toLowerCase().includes(labelQuery.trim().toLowerCase()))
      .slice(0, 8),
  );

  async function labelRow(id: string, png: string, passiveKey: string) {
    try {
      await invoke("save_passive_label", { pngBase64Data: png, passiveKey });
      results =
        results?.map((r) => {
          const hit = r.passive_unknowns.some(([uid]) => uid === id);
          if (!hit) return r;
          return {
            ...r,
            passives:
              passiveKey === "-empty-" ? r.passives : [...r.passives, passiveKey],
            passive_unknowns: r.passive_unknowns.filter(([uid]) => uid !== id),
          };
        }) ?? null;
      labeling = null;
      labelQuery = "";
    } catch (e) {
      error = String(e);
    }
  }

  // Debug tools
  interface DebugSlot { row: number; col: number; occupied: boolean; crop_png: string }
  interface SheetDebug {
    log: string[];
    name_band_png: string | null;
    passives_png: string | null;
    species: string | null;
    name_score: number;
    passives: string[];
    gender: Gender | null;
    panel_png: string | null;
    panel_rect: [number, number, number, number] | null;
    zones_used: [string, [number, number, number, number], boolean][];
  }
  let debugLog = $state<string[]>([]);
  let debugReportPath = $state<string | null>(null);
  let debugSlots = $state<DebugSlot[] | null>(null);
  let debugSheet = $state<SheetDebug | null>(null);
  let debugRunning = $state(false);

  async function runDebug(kind: "grid" | "sheet") {
    debugRunning = true;
    debugLog = [`waiting 2s — switch to the game${kind === "sheet" ? " and hover a pal" : ""}…`];
    debugSlots = null;
    debugSheet = null;
    try {
      if (kind === "grid") {
        const r = await invoke<{ log: string[]; slots: DebugSlot[]; report_path: string }>(
          "debug_grid_capture",
        );
        debugLog = r.log;
        debugSlots = r.slots;
        debugReportPath = r.report_path;
      } else {
        const r = await invoke<SheetDebug & { report_path: string }>("debug_sheet_read");
        debugLog = r.log;
        debugSheet = r;
        debugReportPath = r.report_path;
      }
    } catch (e) {
      debugLog = [...debugLog, `ERROR: ${e}`];
    } finally {
      debugRunning = false;
    }
  }

  // Zone override editor: drag a rect on the panel capture, assign it to an
  // aspect, save, then re-run the sheet test to see reads with the override.
  const ZONE_COLORS: Record<string, string> = {
    name: "#4aa3ff",
    gender: "#ff6bd6",
    passives: "#ffd34a",
    panel: "#4ade80",
  };
  let zoneAspect = $state<"name" | "gender" | "passives" | "panel">("gender");
  let zoneSel = $state<{ x: number; y: number; w: number; h: number } | null>(null);
  let zoneDrag: { x: number; y: number } | null = null;
  let panelImgEl = $state<HTMLImageElement | null>(null);

  // Full-screen zone editor in the main calibration flow: capture the screen,
  // zoom in, draw a rect, assign it to an aspect. Reuses the same drag state
  // and save path as the debug editor; only the coordinate origin differs.
  let frame = $state<FrozenFrame | null>(null);
  let frameImgEl = $state<HTMLImageElement | null>(null);
  let frameCapturing = $state(false);
  let zoom = $state(1);

  // Default zone ratios — MUST match panel.rs (name_rect / gender_rect /
  // passives_search_rect). Shown as dashed guides so a fresh user has a target.
  const DEFAULT_ZONE_RATIOS: Record<string, Rect> = {
    name: [0.2335, 0.0254, 0.5956, 0.0376],
    gender: [0.9063, 0.0206, 0.0683, 0.0359],
    passives: [0.0423, 0.8911, 0.9201, 0.0826],
  };
  function defaultZoneRect(key: string): Rect | null {
    if (!calib.panel) return null;
    const [px, py, pw, ph] = calib.panel;
    const [rx, ry, rw, rh] = DEFAULT_ZONE_RATIOS[key];
    return [
      Math.round(px + rx * pw),
      Math.round(py + ry * ph),
      Math.round(rw * pw),
      Math.round(rh * ph),
    ];
  }

  // Stored zone overrides are FRACTIONS of the panel; resolve to an absolute
  // screen rect for overlays. Null if no override or no panel.
  function zoneAbs(key: string): Rect | null {
    const z = calib.zones[key];
    if (!z || !calib.panel) return null;
    const [px, py, pw, ph] = calib.panel;
    const [fx, fy, fw, fh] = z;
    return [
      Math.round(px + fx * pw),
      Math.round(py + fy * ph),
      Math.round(fw * pw),
      Math.round(fh * ph),
    ];
  }

  function editorPos(e: MouseEvent): { x: number; y: number } {
    const el = e.currentTarget as HTMLElement;
    const b = el.getBoundingClientRect();
    return { x: e.clientX - b.left, y: e.clientY - b.top };
  }
  function zoneDown(e: MouseEvent) {
    e.preventDefault();
    zoneDrag = editorPos(e);
    zoneSel = null;
  }
  function zoneMove(e: MouseEvent) {
    if (!zoneDrag) return;
    const p = editorPos(e);
    zoneSel = {
      x: Math.min(zoneDrag.x, p.x),
      y: Math.min(zoneDrag.y, p.y),
      w: Math.abs(p.x - zoneDrag.x),
      h: Math.abs(p.y - zoneDrag.y),
    };
  }
  function zoneUp() {
    zoneDrag = null;
    if (zoneSel && (zoneSel.w < 4 || zoneSel.h < 4)) zoneSel = null;
  }

  // Convert the current drag selection (display px on `imgEl`) into an absolute
  // screen rect within origin frame `(ox,oy,ow,oh)`.
  function selToScreenRect(
    origin: Rect,
    imgEl: HTMLImageElement,
  ): Rect | null {
    if (!zoneSel) return null;
    const [ox, oy, ow, oh] = origin;
    const sx = ow / imgEl.clientWidth;
    const sy = oh / imgEl.clientHeight;
    return [
      Math.round(ox + zoneSel.x * sx),
      Math.round(oy + zoneSel.y * sy),
      Math.max(1, Math.round(zoneSel.w * sx)),
      Math.max(1, Math.round(zoneSel.h * sy)),
    ];
  }

  // Persist the drawn selection. The pal-sheet PANEL is stored in calib.panel
  // (via save_calibration); every other aspect is a reading-zone override
  // stored via save_zone.
  async function commitZone(
    origin: Rect | null,
    imgEl: HTMLImageElement | null,
    log: (m: string) => void,
  ) {
    if (!origin || !imgEl) return;
    const rect = selToScreenRect(origin, imgEl);
    if (!rect) return;
    try {
      if (zoneAspect === "panel") {
        calib.panel = rect;
        await invoke("save_calibration", { calib });
        calibSaved = true;
        log(`panel bounds set (${rect.join(", ")})`);
      } else {
        await invoke("save_zone", { key: zoneAspect, rect });
        calib.zones = { ...calib.zones, [zoneAspect]: rect };
        log(`zone '${zoneAspect}' saved (${rect.join(", ")})`);
      }
      zoneSel = null;
    } catch (e) {
      log(`ERROR saving ${zoneAspect}: ${e}`);
    }
  }

  async function saveZone() {
    await commitZone(debugSheet?.panel_rect ?? null, panelImgEl, (m) => {
      debugLog = [...debugLog, `${m} — run the sheet test again`];
    });
  }

  // Percent-position a screen rect inside an origin rect for overlay boxes.
  function rectPct(rect: Rect, origin: Rect | null): string {
    if (!origin) return "";
    const [ox, oy, ow, oh] = origin;
    return `left:${((rect[0] - ox) / ow) * 100}%;top:${((rect[1] - oy) / oh) * 100}%;width:${(rect[2] / ow) * 100}%;height:${(rect[3] / oh) * 100}%;`;
  }
  function zonePct(rect: Rect): string {
    return rectPct(rect, debugSheet?.panel_rect ?? null);
  }

  async function captureFrame() {
    frameCapturing = true;
    error = null;
    for (let i = 3; i > 0; i--) {
      countdown = i;
      await sleep(1000);
    }
    countdown = 0;
    try {
      frame = await invoke<FrozenFrame>("capture_screen");
      zoom = 1;
      zoneSel = null;
    } catch (e) {
      error = String(e);
    } finally {
      frameCapturing = false;
    }
  }

  async function saveFrameZone() {
    if (!frame) return;
    await commitZone([frame.x, frame.y, frame.w, frame.h], frameImgEl, (m) => {
      error = null;
      debugLog = [...debugLog, m];
    });
  }

  async function clearZoneMain(key: string) {
    try {
      await invoke("save_zone", { key, rect: null });
      const { [key]: _drop, ...rest } = calib.zones;
      calib.zones = rest;
    } catch (e) {
      error = String(e);
    }
  }

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
      <span class="hint-inline">— resets panel + all zones, or calibrate manually below</span>
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

    <h4>Reading zones (precise, zoomable)</h4>
    <p class="dim-text">
      Hover a pal in-game, capture the screen, zoom in, and drag a tight box.
      Pick <strong>panel</strong> to set the whole pal-sheet bounds, or
      <strong style={`color:${ZONE_COLORS[zoneAspect]}`}>{zoneAspect}</strong>
      for a single reading zone. Dashed boxes show the auto defaults; solid
      boxes are your overrides.
    </p>
    <div class="row">
      <button onclick={captureFrame} disabled={frameCapturing || !status?.backend}>
        {frameCapturing && countdown ? `…${countdown}` : "Capture screen for zones"}
      </button>
      <select bind:value={zoneAspect}>
        <option value="panel">panel (sheet bounds)</option>
        <option value="name">name</option>
        <option value="gender">gender</option>
        <option value="passives">passives</option>
      </select>
      {#if frame}
        <label>
          zoom {zoom.toFixed(1)}×
          <input type="range" min="1" max="8" step="0.5" bind:value={zoom} />
        </label>
        <button onclick={() => (zoom = 1)}>Fit</button>
        <button class="save" onclick={saveFrameZone} disabled={!zoneSel}>
          Save {zoneAspect === "panel" ? "panel bounds" : `${zoneAspect} zone`}
        </button>
      {/if}
    </div>
    {#if frame}
      <div class="row">
        {#each ["name", "gender", "passives"] as key}
          {#if calib.zones[key]}
            <button onclick={() => clearZoneMain(key)}>Clear {key} override</button>
          {/if}
        {/each}
      </div>
      <div class="zoom-viewport">
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="zoom-stage"
          style={`width:${zoom * 100}%`}
          onmousedown={zoneDown}
          onmousemove={zoneMove}
          onmouseup={zoneUp}
          onmouseleave={zoneUp}
        >
          <img
            bind:this={frameImgEl}
            src={frame.data_url}
            alt="screen"
            draggable="false"
          />
          {#if calib.panel}
            <div
              class="zone-box"
              style={`${rectPct(calib.panel, [frame.x, frame.y, frame.w, frame.h])}border-color:${ZONE_COLORS.panel};`}
              title="panel (sheet bounds)"
            >
              <span style={`background:${ZONE_COLORS.panel}`}>panel</span>
            </div>
          {/if}
          {#each ["name", "gender", "passives"] as key}
            {@const def = defaultZoneRect(key)}
            {#if def && !calib.zones[key]}
              <div
                class="zone-box dashed"
                style={`${rectPct(def, [frame.x, frame.y, frame.w, frame.h])}border-color:${ZONE_COLORS[key]};`}
                title={`${key} (default)`}
              >
                <span style={`background:${ZONE_COLORS[key]}`}>{key}</span>
              </div>
            {/if}
            {#if zoneAbs(key)}
              <div
                class="zone-box"
                style={`${rectPct(zoneAbs(key)!, [frame.x, frame.y, frame.w, frame.h])}border-color:${ZONE_COLORS[key]};`}
                title={`${key} (override)`}
              >
                <span style={`background:${ZONE_COLORS[key]}`}>{key} ✎</span>
              </div>
            {/if}
          {/each}
          {#if zoneSel}
            <div
              class="zone-sel"
              style={`left:${zoneSel.x}px;top:${zoneSel.y}px;width:${zoneSel.w}px;height:${zoneSel.h}px;border-color:${ZONE_COLORS[zoneAspect]};`}
            ></div>
          {/if}
        </div>
      </div>
    {/if}
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

  <details>
    <summary>Debug tools</summary>
    <div class="row">
      <button onclick={() => runDebug("grid")} disabled={debugRunning}>
        Test empty detection (2s)
      </button>
      <button onclick={() => runDebug("sheet")} disabled={debugRunning}>
        Test sheet read (2s) — hover a pal first
      </button>
    </div>
    {#if debugReportPath}
      <p class="dim-text">
        Shareable bundle written to <code>{debugReportPath}</code> — pass it on
        with:<br />
        <code>cp -r {debugReportPath} ~/Projects/palCalc/gaming-debug && cd
        ~/Projects/palCalc && git add gaming-debug && git commit -m debug &&
        git push</code>
      </p>
    {/if}
    {#if debugLog.length > 0}
      <pre class="debug-log">{debugLog.join("\n")}</pre>
    {/if}
    {#if debugSlots}
      <div class="debug-grid" style={`grid-template-columns: repeat(${calib.cols}, 48px)`}>
        {#each debugSlots as d (d.row * 100 + d.col)}
          <div class="debug-slot" class:occupied={d.occupied} title={`(${d.row},${d.col}) ${d.occupied ? "occupied" : "empty"}`}>
            <img src={"data:image/png;base64," + d.crop_png} alt="" />
          </div>
        {/each}
      </div>
    {/if}
    {#if debugSheet}
      <div class="debug-sheet">
        <p>
          species: <strong>{debugSheet.species ? (pal(debugSheet.species)?.name ?? debugSheet.species) : "—"}</strong>
          ({debugSheet.name_score.toFixed(3)})
          {genderSymbol(debugSheet.gender)}
          · passives: {debugSheet.passives.map(passiveName).join(", ") || "none"}
        </p>
        {#if debugSheet.name_band_png}
          <p class="dim-text">name band capture:</p>
          <img class="debug-img" src={"data:image/png;base64," + debugSheet.name_band_png} alt="name band" />
        {/if}
        {#if debugSheet.passives_png}
          <p class="dim-text">passives region capture:</p>
          <img class="debug-img" src={"data:image/png;base64," + debugSheet.passives_png} alt="passives region" />
        {/if}
        {#if debugSheet.panel_png && debugSheet.panel_rect}
          <p class="dim-text">
            panel capture with the zones the read used — drag on the image to
            propose a better <strong style={`color:${ZONE_COLORS[zoneAspect]}`}>{zoneAspect}</strong>
            zone, save it, then run the sheet test again:
          </p>
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="zone-editor"
            onmousedown={zoneDown}
            onmousemove={zoneMove}
            onmouseup={zoneUp}
            onmouseleave={zoneUp}
          >
            <img
              bind:this={panelImgEl}
              src={"data:image/png;base64," + debugSheet.panel_png}
              alt="panel"
              draggable="false"
            />
            {#each debugSheet.zones_used as [key, rect, ovr] (key)}
              <div
                class="zone-box"
                style={`${zonePct(rect)}border-color:${ZONE_COLORS[key] ?? "#fff"};`}
                title={`${key}${ovr ? " (override)" : ""}`}
              >
                <span style={`background:${ZONE_COLORS[key] ?? "#fff"}`}>{key}{ovr ? " ✎" : ""}</span>
              </div>
            {/each}
            {#if zoneSel}
              <div
                class="zone-sel"
                style={`left:${zoneSel.x}px;top:${zoneSel.y}px;width:${zoneSel.w}px;height:${zoneSel.h}px;border-color:${ZONE_COLORS[zoneAspect]};`}
              ></div>
            {/if}
          </div>
          <div class="row">
            <select bind:value={zoneAspect}>
              <option value="name">name</option>
              <option value="gender">gender</option>
              <option value="passives">passives</option>
            </select>
            <button onclick={saveZone} disabled={!zoneSel}>Save {zoneAspect} zone</button>
            {#each debugSheet.zones_used.filter((z) => z[2]) as [key] (key)}
              <button onclick={() => clearZoneMain(key)}>Clear {key} override</button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </details>

  {#if unknownRows.length > 0}
    <div class="label-panel">
      <h4>Unrecognized passive rows — label once, matched exactly forever</h4>
      {#each unknownRows as u (u.id)}
        <div class="unknown">
          <img src={"data:image/png;base64," + u.png} alt="passive row" />
          {#if labeling === u.id}
            <input placeholder="Type passive name…" bind:value={labelQuery} />
            {#each labelMatches as m (m.key)}
              <button class="pick" onclick={() => labelRow(u.id, u.png, m.key)}>{m.name}</button>
            {/each}
            <button class="pick empty-pick" onclick={() => labelRow(u.id, u.png, "-empty-")}>
              Not a passive
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

  .debug-log {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.6rem 0.8rem;
    font-size: 0.78rem;
    max-height: 260px;
    overflow: auto;
    white-space: pre-wrap;
  }

  .debug-grid {
    display: grid;
    gap: 3px;
    margin-top: 0.5rem;
  }

  .debug-slot {
    border: 2px solid #f87171;
    border-radius: 6px;
    opacity: 0.55;
  }

  .debug-slot.occupied {
    border-color: #4ade80;
    opacity: 1;
  }

  .debug-slot img {
    width: 100%;
    display: block;
  }

  .debug-img {
    max-width: 100%;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: #000;
  }

  .zone-editor {
    position: relative;
    display: inline-block;
    max-width: 100%;
    cursor: crosshair;
    user-select: none;
  }
  .zone-editor img {
    display: block;
    max-width: 100%;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: #000;
    pointer-events: none;
  }
  .zone-box {
    position: absolute;
    border: 2px solid;
    pointer-events: none;
    box-sizing: border-box;
  }
  .zone-box span {
    position: absolute;
    top: -1.1rem;
    left: 0;
    font-size: 0.65rem;
    color: #000;
    padding: 0 0.25rem;
    border-radius: 3px;
    white-space: nowrap;
  }
  .zone-sel {
    position: absolute;
    border: 2px dashed;
    pointer-events: none;
    box-sizing: border-box;
  }

  /* Zoomable full-screen zone editor */
  .zoom-viewport {
    overflow: auto;
    max-height: 70vh;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: #000;
    margin-top: 0.5rem;
  }
  .zoom-stage {
    position: relative;
    cursor: crosshair;
    user-select: none;
    min-width: 100%;
  }
  .zoom-stage img {
    display: block;
    width: 100%;
    pointer-events: none;
  }
  .zone-box.dashed {
    border-style: dashed;
    opacity: 0.7;
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
    max-width: 320px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: #000;
  }

  .unknown input {
    padding: 0.35rem 0.6rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
  }

  .empty-pick {
    color: var(--text-dim);
    border-style: dashed;
  }

  .add-all {
    align-self: flex-start;
    background: var(--accent);
    color: #1a1408;
    font-weight: 600;
    border: none;
  }
</style>

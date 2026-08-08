<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { addManyOwned, clearAllOwned, ownedStore, replaceAllOwned } from "./owned.svelte";
  import type { Gender, PalEntry, PassiveEntry } from "./types";

  type Rect = [number, number, number, number];

  interface Calib {
    slot_tl: [number, number];
    slot_br: [number, number];
    cols: number;
    rows: number;
    slot_size: number;
    /// Hover settle. With adaptive_delay on this is the CEILING, not a fixed wait.
    delay_ms: number;
    grid_unhover_ms: number;
    first_slot_ms: number;
    box_settle_ms: number;
    /// Poll the panel until it has repainted instead of always waiting delay_ms.
    adaptive_delay: boolean;
    /// Floor before the first poll when adaptive_delay is on.
    min_delay_ms: number;
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
    valid: boolean;
  }

  interface SlotResult {
    box_index: number;
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
    // Mirrors the Rust defaults in GridCalibration::default(); this literal is
    // only used before the backend calibration loads.
    delay_ms: 60,
    grid_unhover_ms: 20,
    first_slot_ms: 50,
    box_settle_ms: 150,
    adaptive_delay: true,
    min_delay_ms: 20,
    panel: null,
    zones: {},
  });
  let calibSaved = $state(false);
  // Whether the calibration <details> is expanded. Deliberately NOT derived
  // from calibSaved: saving a zone mid-calibration must not snap the panel
  // shut (and capturing a corner must not pop it open). Initialized once
  // from the loaded status, then only the user toggles it.
  let calibOpen = $state(true);
  let countdown = $state(0);
  let capturing = $state<0 | 1 | 2 | 3 | 4>(0);
  let scanning = $state(false);
  let progress = $state<{
    current: number;
    total: number;
    box_current?: number;
    box_total?: number;
  } | null>(null);
  let results = $state<SlotResult[] | null>(null);
  let error = $state<string | null>(null);

  // Species correction: teaches the matcher this game's own rendering
  let fixing = $state<string | null>(null); // "boxIndex,row,col"
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

  // Dump & label system for replay testing
  interface DumpInfo { path: string; timestamp: string; has_labels: boolean; slot_count: number }
  interface SlotLabel { species: string | null; passives: string[]; gender: string | null }
  let dumps = $state<DumpInfo[]>([]);
  let selectedDump = $state<string | null>(null);
  let dumpLabels = $state<Record<string, SlotLabel>>({});
  let savingDump = $state(false);

  // Passive dropdown state
  let passiveSearch = $state("");
  let activePassiveSlot = $state<string | null>(null);
  let passiveIdx = $state(-1);

  $effect(() => {
    passiveIdx = passiveSearch ? 0 : -1;
  });

  function filteredPassives(exclude: string[]): PassiveEntry[] {
    const q = passiveSearch.toLowerCase();
    return passiveList.filter(
      (p) => !exclude.includes(p.name) && (!q || p.name.toLowerCase().includes(q)),
    );
  }

  function addPassive(slotKey: string, name: string) {
    const lbl = dumpLabels[slotKey];
    if (lbl && !lbl.passives.includes(name)) {
      dumpLabels[slotKey] = { ...lbl, passives: [...lbl.passives, name] };
    }
    passiveSearch = "";
    passiveIdx = -1;
  }

  function removePassive(slotKey: string, idx: number) {
    const lbl = dumpLabels[slotKey];
    if (!lbl) return;
    dumpLabels[slotKey] = {
      ...lbl,
      passives: lbl.passives.filter((_, i) => i !== idx),
    };
  }

  function passiveDropdownKeydown(e: KeyboardEvent, slotKey: string) {
    const opts = filteredPassives(dumpLabels[slotKey]?.passives ?? []);
    if (e.key === "ArrowDown") {
      e.preventDefault();
      passiveIdx = Math.min(passiveIdx + 1, opts.length - 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      passiveIdx = Math.max(passiveIdx - 1, 0);
    } else if (e.key === "Enter" && passiveIdx >= 0 && passiveIdx < opts.length) {
      e.preventDefault();
      addPassive(slotKey, opts[passiveIdx].name);
    } else if (e.key === "Escape") {
      passiveSearch = "";
      passiveIdx = -1;
    }
  }

  function speciesDisplayName(key: string | null): string {
    if (!key) return "";
    return pals.find(p => p.key === key)?.name ?? key;
  }

  function computeLabels(results: SlotResult[], rows: number, cols: number): Record<string, SlotLabel> {
    const labels: Record<string, SlotLabel> = {};
    for (const r of results) {
      labels[`${r.box_index},${r.row},${r.col}`] = {
        species: r.species,
        passives: r.passives,
        gender: r.gender ? String(r.gender) : null,
      };
    }
    // Fill empty slots for all boxes present in the results.
    const maxBox = results.reduce((m, r) => Math.max(m, r.box_index), 0);
    for (let b = 0; b <= maxBox; b++) {
      for (let row = 0; row < rows; row++) {
        for (let col = 0; col < cols; col++) {
          const key = `${b},${row},${col}`;
          if (!labels[key]) labels[key] = { species: null, passives: [], gender: null };
        }
      }
    }
    return labels;
  }

  async function refreshDumps() {
    dumps = await invoke<DumpInfo[]>("list_dumps");
  }

  async function saveForReplay() {
    if (!results || results.length === 0) return;
    savingDump = true;
    try {
      const labels = computeLabels(results, calib.rows, calib.cols);
      const path = await invoke<string>("save_last_scan_for_replay", { labels });
      selectedDump = path;
      dumpLabels = labels;
      await refreshDumps();
    } catch (e) {
      error = String(e);
    } finally {
      savingDump = false;
    }
  }

  async function loadDumpLabels(path: string) {
    selectedDump = path;
    try {
      const raw = await invoke<Record<string, SlotLabel>>("load_dump_labels", { path });
      // Convert internal passive keys to display names for the editor.
      const converted: Record<string, SlotLabel> = {};
      for (const [k, lbl] of Object.entries(raw)) {
        converted[k] = {
          ...lbl,
          passives: lbl.passives.map((key) => passiveName(key)),
        };
      }
      dumpLabels = converted;
    } catch (e) {
      error = String(e);
    }
  }

  async function saveDumpLabels() {
    if (!selectedDump) return;
    // Convert passive display names back to internal keys for storage.
    const toSave: Record<string, SlotLabel> = {};
    for (const [k, lbl] of Object.entries(dumpLabels)) {
      toSave[k] = {
        ...lbl,
        passives: lbl.passives.map((name) => {
          const match = passiveList.find((p) => p.name === name);
          return match ? match.key : name;
        }),
      };
    }
    try {
      await invoke("save_dump_labels", { path: selectedDump, labels: toSave });
    } catch (e) {
      error = String(e);
    }
  }

  // Zone override editor: drag a rect on the panel capture, assign it to an
  // aspect, save, then re-run the sheet test to see reads with the override.
  const ZONE_COLORS: Record<string, string> = {
    name: "#4aa3ff",
    gender: "#ff6bd6",
    passives: "#ffd34a",
    "passive 1": "#ff6b6b",
    "passive 2": "#6bff6b",
    "passive 3": "#6b6bff",
    "passive 4": "#ffff6b",
    panel: "#4ade80",
  };
  let zoneAspect = $state<"name" | "gender" | "passives" | "passive 1" | "passive 2" | "passive 3" | "passive 4" | "panel">("gender");
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
    "passive 1": [0.0423, 0.8911, 0.4550, 0.0380],
    "passive 2": [0.5074, 0.8911, 0.4550, 0.0380],
    "passive 3": [0.0423, 0.9331, 0.4550, 0.0380],
    "passive 4": [0.5074, 0.9331, 0.4550, 0.0380],
  };
  function defaultZoneRect(key: string): Rect | null {
    if (!calib.panel) return null;
    const r = DEFAULT_ZONE_RATIOS[key];
    if (!r) return null;
    const [px, py, pw, ph] = calib.panel;
    const [rx, ry, rw, rh] = r;
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
        await refreshStatus();
        log(`panel bounds set (${rect.join(", ")})`);
      } else {
        await invoke("save_zone", { key: zoneAspect, rect });
        if (calib.panel) {
          const [px, py, pw, ph] = calib.panel;
          calib.zones = { ...calib.zones, [zoneAspect]: [
            (rect[0] - px) / pw, (rect[1] - py) / ph,
            rect[2] / pw, rect[3] / ph,
          ]};
        }
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
    try {
      pals = await invoke<PalEntry[]>("list_pals");
      passiveList = await invoke<PassiveEntry[]>("list_passives");
      status = await invoke<ScannerStatus>("scanner_status");
    if (status.calibration) {
      calib = status.calibration;
      calibSaved = true;
      calibOpen = false;
    }
    await listen<{
      current: number;
      total: number;
      box_current?: number;
      box_total?: number;
    }>("scan-progress", (e) => {
      progress = e.payload;
    });
    } catch (e) {
      error = String(e);
    }
  });

  async function refreshStatus() {
    try {
      status = await invoke<ScannerStatus>("scanner_status");
      if (status.calibration) {
        calib = status.calibration;
      }
    } catch (e) {
      error = String(e);
    }
  }

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
      await refreshStatus();
    } catch (e) {
      error = String(e);
    }
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
    } catch (e) {
      error = String(e);
    }
    capturing = 0;
  }

  // Persist the adaptive-settle toggle immediately — it sits in Setup with no
  // save button of its own. Before any calibration exists there is nothing
  // meaningful to persist; the next save/defaults will carry it.
  async function saveAdaptiveToggle() {
    if (!calibSaved) return;
    try {
      await invoke("save_calibration", { calib });
    } catch (e) {
      error = String(e);
    }
  }

  async function saveCalib() {
    try {
      await invoke("save_calibration", { calib });
      calibSaved = true;
      error = null;
      await refreshStatus();
    } catch (e) {
      error = String(e);
    }
  }

  let scanReportPath = $state<string | null>(null);

  async function runScan(command: "scan_current_box" | "scan_all_boxes") {
    scanning = true;
    error = null;
    results = null;
    scanReportPath = null;
    progress = null;
    try {
      const r = await invoke<{ slots: SlotResult[]; report_path: string }>(
        command,
        {},
      );
      results = r.slots;
      scanReportPath = r.report_path;
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
  }
  // A full sweep REPLACES the owned list (the confirm below promises as
  // much); a single-box scan appends to it.
  let replaceOnAdd = false;
  const scan = () => {
    replaceOnAdd = false;
    runScan("scan_current_box");
  };
  function scanAll() {
    if (ownedStore.list.length > 0 && !confirm(`You have ${ownedStore.list.length} owned pals. Scanning will replace them. Continue?`)) return;
    replaceOnAdd = true;
    runScan("scan_all_boxes");
  }

  const found = $derived(results?.filter((r) => r.species !== null) ?? []);

  const fixMatches = $derived(
    pals
      .filter((p) => p.name.toLowerCase().includes(fixQuery.trim().toLowerCase()))
      .slice(0, 8),
  );

  // Find a result by composite key.
  function findSlot(key: string): SlotResult | undefined {
    const [bi, row, col] = key.split(",").map(Number);
    return results?.find((r) => r.box_index === bi && r.row === row && r.col === col);
  }

  async function fixSpecies(key: string, speciesKey: string) {
    const r = findSlot(key);
    if (!r) return;
    try {
      await invoke("save_pal_template", {
        pngBase64Data: r.crop_png,
        species: speciesKey,
      });
      results = results!.map((s) =>
        s.box_index === r.box_index && s.row === r.row && s.col === r.col
          ? { ...s, species: speciesKey, score: 1.0 }
          : s,
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
    (replaceOnAdd ? replaceAllOwned : addManyOwned)(
      found.map((r) => ({
        species: r.species!,
        label: `${pal(r.species)?.name ?? r.species} (scan)`,
        passives: r.passives,
        gender: r.gender,
      })),
    );
    results = null;
    scanReportPath = null;
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

  <details bind:open={calibOpen}>
    <summary>Setup {calibSaved ? "✓" : "(needed before scanning)"}</summary>
    <p class="dim-text">
      One click sets the standard 16:9 layout scaled to your monitor. Then
      open the Palbox in Palworld, hover any pal, and run a test read — if
      species, gender and passives come back right, you're ready to scan.
    </p>
    <div class="row">
      <button class="save" onclick={useDefaults}>Set up for my monitor</button>
      <button onclick={() => runDebug("sheet")} disabled={debugRunning || savingDump || !calibSaved}>
        {debugRunning ? "Reading…" : "Test read (2s) — hover a pal first"}
      </button>
      <label title="Poll the panel until it has visibly repainted instead of always waiting the full delay. Falls back to the full wait whenever the change can't be observed, so it never risks reading a stale panel.">
        <input type="checkbox" bind:checked={calib.adaptive_delay} onchange={saveAdaptiveToggle} />
        adaptive settle (experimental)
      </label>
    </div>
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
            {#each ["passive 1", "passive 2", "passive 3", "passive 4"] as key}
              {@const def = defaultZoneRect(key)}
              {#if def && !debugSheet.zones_used.find((z) => z[0] === key)}
                <div
                  class="zone-box dashed"
                  style={`${zonePct(def)}border-color:${ZONE_COLORS[key]};`}
                  title={`${key} (default)`}
                >
                  <span style={`background:${ZONE_COLORS[key]}`}>{key}</span>
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
          <div class="row">
            <select bind:value={zoneAspect}>
              <option value="name">name</option>
              <option value="gender">gender</option>
              <option value="passives">passives (full grid)</option>
              <option value="passive 1">passive 1 (top-left)</option>
              <option value="passive 2">passive 2 (top-right)</option>
              <option value="passive 3">passive 3 (bottom-left)</option>
              <option value="passive 4">passive 4 (bottom-right)</option>
            </select>
            <button onclick={saveZone} disabled={!zoneSel}>Save {zoneAspect} zone</button>
            {#each debugSheet.zones_used.filter((z) => z[2]) as [key] (key)}
              <button onclick={() => clearZoneMain(key)}>Clear {key} override</button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    <details>
      <summary>Grid doesn't match? Capture the slot corners manually</summary>
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
    </details>
    <details>
      <summary>Advanced — grid dimensions, timings, reading zones</summary>
    <div class="row">
      <label>cols <input type="number" min="1" bind:value={calib.cols} /></label>
      <label>rows <input type="number" min="1" bind:value={calib.rows} /></label>
      <label>slot px <input type="number" min="20" bind:value={calib.slot_size} /></label>
      <button class="save" onclick={saveCalib}>Save calibration</button>
    </div>
    <div class="row">
      <label title="Hover settle. With adaptive settle on this is the CEILING — the scan waits at most this long.">
        {calib.adaptive_delay ? "delay ceiling" : "delay"}
        <input type="number" min="10" max="1000" step="10" bind:value={calib.delay_ms} /> ms
      </label>
      {#if calib.adaptive_delay}
        <label title="Floor before the first poll: the game needs some time to begin repainting, and polling before that just wastes captures.">
          floor
          <input type="number" min="0" max="200" step="5" bind:value={calib.min_delay_ms} /> ms
        </label>
      {/if}
    </div>
    <div class="row">
      <label title="Wait after parking the cursor off-grid before the unhovered grid capture.">
        grid unhover
        <input type="number" min="0" max="500" step="5" bind:value={calib.grid_unhover_ms} /> ms
      </label>
      <label title="Wait on the first occupied slot of each box — the panel has to appear from scratch, so it always waits in full.">
        first slot
        <input type="number" min="0" max="1000" step="10" bind:value={calib.first_slot_ms} /> ms
      </label>
      <label title="Wait after pressing E to switch boxes; the page change animates.">
        box settle
        <input type="number" min="0" max="1000" step="10" bind:value={calib.box_settle_ms} /> ms
      </label>
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
        <option value="passives">passives (full grid)</option>
        <option value="passive 1">passive 1 (top-left)</option>
        <option value="passive 2">passive 2 (top-right)</option>
        <option value="passive 3">passive 3 (bottom-left)</option>
        <option value="passive 4">passive 4 (bottom-right)</option>
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
        {#each ["name", "gender", "passives", "passive 1", "passive 2", "passive 3", "passive 4"] as key}
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
        {#each ["name", "gender", "passives", "passive 1", "passive 2", "passive 3", "passive 4"] as key}
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
  </details>

  <div class="row scan-row">
    <button class="scan" onclick={scan} disabled={scanning || savingDump || !calibSaved || !status?.backend || !status?.valid}>
      {scanning ? "Scanning…" : "Scan current box"}
    </button>
    <button class="scan" onclick={scanAll} disabled={scanning || savingDump || !calibSaved || !status?.backend || !status?.valid}>
      Scan all 32 boxes
    </button>
    {#if ownedStore.list.length > 0}
      <button class="remove-all" onclick={() => { if (confirm("Remove all owned pals? This cannot be undone.")) clearAllOwned(); }} disabled={scanning}>
        Remove all pals
      </button>
    {/if}
    {#if scanning}
      <button onclick={() => invoke("abort_scan").catch(() => {})}>Abort</button>
      {#if progress}
        <progress value={progress.current} max={progress.total}></progress>
        <span class="pos">
          {#if progress.box_total && progress.box_total > 1}
            box {progress.box_current}/{progress.box_total} ·
          {/if}
          {progress.current} / {progress.total}
        </span>
      {/if}
    {/if}
    <span class="hint-inline">
      "Scan all" presses E to page through every box — open the palbox on box 1
      and keep Palworld focused. Results accumulate in Owned Pals.
    </span>
  </div>

  {#if scanReportPath}
    <p class="dim-text">
      Scan debug bundle written to <code>{scanReportPath}</code> — pass it on
      with:<br />
      <code>rm -rf ~/Projects/palCalc/gaming-debug/debug-report && cp -r
      {scanReportPath} ~/Projects/palCalc/gaming-debug && cd
      ~/Projects/palCalc && git add gaming-debug && git commit -m debug &&
      git push</code>
    </p>
  {/if}

  {#if error}
    <p class="banner error">{error}</p>
  {/if}

  <details>
    <summary>Debug tools</summary>
    <div class="row">
      <button onclick={() => runDebug("grid")} disabled={debugRunning || savingDump}>
        Test empty detection (2s)
      </button>
    </div>
    {#if debugReportPath}
      <p class="dim-text">
        Shareable bundle written to <code>{debugReportPath}</code> — pass it on
        with:<br />
        <code>rm -rf ~/Projects/palCalc/gaming-debug/debug-report && cp -r
        {debugReportPath} ~/Projects/palCalc/gaming-debug && cd
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
    <!-- Dump & replay testing -->
    <hr />
    <h4>Validate scan</h4>
    <p class="dim-text">Confirm the scan results are correct and save for offline replay testing.</p>
    <div class="row">
      <button onclick={saveForReplay} disabled={savingDump || !results || results.length === 0}>
        {savingDump ? "Saving…" : "Scan was correct"}
      </button>
      <button onclick={refreshDumps} disabled={savingDump}>Refresh list</button>
    </div>
    {#if selectedDump}
      <p class="dim-text">Saved to <code>{selectedDump}</code></p>
    {/if}
    {#if dumps.length > 0}
      <details>
        <summary>Saved dumps ({dumps.length})</summary>
        {#each dumps as d (d.path)}
          <div class="row" style="align-items:center; gap:0.5rem;">
            <code style="font-size:0.75rem;">{d.timestamp}</code>
            <span class="dim-text">{d.slot_count} slots · {d.has_labels ? "labeled" : "needs labels"}</span>
            <button onclick={() => loadDumpLabels(d.path)}>Load</button>
            <button class="danger" onclick={() => { invoke("delete_dump", { path: d.path }).then(refreshDumps) }}>Delete</button>
          </div>
        {/each}
      </details>
    {/if}
    {#if Object.keys(dumpLabels).length > 0}
      <details open>
        <summary>Edit labels ({Object.keys(dumpLabels).length} slots)</summary>
        <div class="label-grid" style="grid-template-columns: repeat(6, minmax(0, 1fr));">
          {#each Object.entries(dumpLabels) as [slotKey, lbl] (slotKey)}
            <div class="label-slot">
              <span class="dim-text" style="font-size:0.65rem;">{slotKey}</span>
              <span class="species-display" title={lbl.species ?? ""}>{speciesDisplayName(lbl.species) || "—"}</span>
              <select
                value={lbl.gender ?? ""}
                onchange={(e) => { const v = (e.target as HTMLSelectElement).value; dumpLabels[slotKey] = { ...lbl, gender: v || null }; }}
              >
                <option value="">gender</option>
                <option value="Male">Male</option>
                <option value="Female">Female</option>
              </select>
              <div class="passive-editor">
                {#each lbl.passives as name, i (name + i)}
                  <span class="passive-tag">
                    {name}
                    <button class="passive-rm" onclick={() => removePassive(slotKey, i)}>×</button>
                  </span>
                {/each}
                <input
                  class="passive-input"
                  placeholder={lbl.passives.length === 0 ? "add passive…" : ""}
                  bind:value={passiveSearch}
                  onfocus={() => { activePassiveSlot = slotKey; }}
                  onblur={() => { setTimeout(() => { activePassiveSlot = null; passiveSearch = ""; }, 150); }}
                  onkeydown={(e) => passiveDropdownKeydown(e, slotKey)}
                />
                {#if activePassiveSlot === slotKey && passiveSearch}
                  {@const opts = filteredPassives(lbl.passives)}
                  {#if opts.length > 0}
                    <ul class="passive-options">
                      {#each opts as p, i (p.key)}
                        <li
                          class="passive-opt"
                          class:active={i === passiveIdx}
                          role="option"
                          aria-selected={i === passiveIdx}
                          onmousedown={() => addPassive(slotKey, p.name)}
                          onmouseenter={() => { passiveIdx = i; }}
                        >{p.name}</li>
                      {/each}
                    </ul>
                  {/if}
                {/if}
              </div>
            </div>
          {/each}
        </div>
        <button onclick={saveDumpLabels}>Save labels</button>
      </details>
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
    <!-- Numeric sort: the default .sort() is lexicographic and orders 32
         boxes as 1, 10, 11, …, 2, 20, … -->
    {#each [...new Set(results.map(r => r.box_index))].sort((a, b) => a - b) as bi}
      {@const box = results.filter(r => r.box_index === bi)}
      {#if results.some(r => r.box_index !== 0)}<p class="dim-text">Box {bi + 1}</p>{/if}
      <!-- Text wraps down inside the cards; tracks bottom out at the longest
           word (nothing clips), and the .results rule centers the rare
           overhang so a too-wide grid grows past the section symmetrically. -->
      <div class="results" style={`grid-template-columns: repeat(${calib.cols}, minmax(min-content, 1fr))`}>
        {#each box as r (r.box_index + "," + r.row + "," + r.col)}
          {@const slotKey = r.box_index + "," + r.row + "," + r.col}
          <div
            class="slot"
            class:empty={!r.species && !r.unidentified}
            style={`grid-row: ${r.row + 1}; grid-column: ${r.col + 1}`}
          >
            {#if fixing === slotKey}
              <img class="crop" src={"data:image/png;base64," + r.crop_png} alt="slot" />
              <div class="fix-box">
                <input placeholder="Correct pal…" bind:value={fixQuery} />
                <div class="fix-options">
                  {#each fixMatches as m (m.key)}
                    <button class="pick" onclick={() => fixSpecies(slotKey, m.key)}>{m.name}</button>
                  {/each}
                </div>
              </div>
              <button class="fix" onclick={() => (fixing = null)}>✕</button>
            {:else}
              {#if r.species}
                {@const p = pal(r.species)}
                {#if p?.icon}<img src={"/icons/" + p.icon} alt="" />{/if}
                <div class="slot-info">
                  <span title={p?.name ?? r.species}>{p?.name ?? r.species} {genderSymbol(r.gender)}</span>
                  <span class="passives" title={r.passives.map(passiveName).join(", ")}>
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
              {#if r.unidentified}
                <button class="fix" title="Correct species" onclick={() => { fixing = slotKey; fixQuery = ""; }}>
                  ✎
                </button>
              {/if}
            {/if}
          </div>
        {/each}
      </div>
    {/each}
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

  .remove-all {
    margin-left: auto;
    color: #ef4444;
    border-color: #ef444444;
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
    gap: 0.4rem;
    /* Size to content, but never narrower than the section; when wider,
       stay centered so the overhang extends equally to both sides. Cap at
       the viewport and scroll within rather than pushing the page wide. */
    width: max-content;
    min-width: 100%;
    max-width: calc(100vw - 3rem);
    overflow-x: auto;
    position: relative;
    left: 50%;
    transform: translateX(-50%);
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

  .danger {
    color: #ef4444;
    border-color: #ef444444;
  }

  hr {
    border: none;
    border-top: 1px solid var(--border);
    margin: 0.5rem 0;
  }

  .label-grid {
    display: grid;
    gap: 0.4rem;
    margin-top: 0.5rem;
  }

  .label-slot {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    padding: 0.35rem;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .label-slot input {
    padding: 0.25rem 0.4rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
    font-size: 0.75rem;
  }

  .label-slot select {
    padding: 0.25rem 0.4rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
    font-size: 0.75rem;
  }

  .species-display {
    padding: 0.25rem 0.4rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
    font-size: 0.75rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .passive-editor {
    display: flex;
    flex-wrap: wrap;
    gap: 0.2rem;
    padding: 0.25rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    min-height: 1.6rem;
    position: relative;
  }

  .passive-tag {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    padding: 0.1rem 0.35rem;
    background: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: 4px;
    font-size: 0.7rem;
    color: var(--text);
  }

  .passive-rm {
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    padding: 0;
    font-size: 0.8rem;
    line-height: 1;
  }

  .passive-rm:hover {
    color: #ef4444;
  }

  .passive-input {
    flex: 1;
    min-width: 4rem;
    border: none;
    background: transparent;
    color: var(--text);
    font-size: 0.7rem;
    padding: 0.15rem 0.2rem;
    outline: none;
  }

  .passive-options {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    z-index: 10;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 4px;
    max-height: 8rem;
    overflow-y: auto;
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .passive-opt {
    padding: 0.3rem 0.5rem;
    cursor: pointer;
    font-size: 0.75rem;
    color: var(--text);
  }

  .passive-opt:hover,
  .passive-opt.active {
    background: var(--bg-hover);
  }
</style>

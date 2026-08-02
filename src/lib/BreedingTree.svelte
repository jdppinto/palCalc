<script lang="ts">
  import { select } from "d3-selection";
  import { zoom, type D3ZoomEvent } from "d3-zoom";
  import type { Gender, Route, RouteNode } from "./types";

  let { route }: { route: Route | null } = $props();

  const NODE_W = 172;
  const NODE_H = 68;
  const GAP_X = 26;
  const GAP_Y = 110;

  interface Laid {
    node: RouteNode;
    id: string;
    x: number;
    y: number;
    depth: number;
    gender: Gender | null;
    collapsed: boolean;
    parentsLaid: Laid[];
  }

  let collapsed = $state<Set<string>>(new Set());
  let selectedChain = $state<Set<string>>(new Set());
  let transform = $state("translate(0,0) scale(1)");
  let svgEl = $state<SVGSVGElement | undefined>();

  // The component stays mounted with route=null (keep-alive tabs), so the
  // zoom behavior binds whenever the svg element actually appears.
  $effect(() => {
    if (!svgEl) return;
    const sel = select(svgEl);
    const z = zoom<SVGSVGElement, unknown>()
      .scaleExtent([0.15, 3])
      .on("zoom", (e: D3ZoomEvent<SVGSVGElement, unknown>) => {
        transform = e.transform.toString();
      });
    sel.call(z);
    return () => { sel.on(".zoom", null); };
  });

  // Reset view state when a different route arrives
  $effect(() => {
    void route;
    collapsed = new Set();
    selectedChain = new Set();
  });

  const layout = $derived.by(() => {
    if (!route) return null;
    let nextLeaf = 0;
    let maxDepth = 0;
    const nodes: Laid[] = [];

    function place(
      n: RouteNode,
      id: string,
      depth: number,
      gender: Gender | null,
    ): Laid {
      maxDepth = Math.max(maxDepth, depth);
      const isCollapsed = collapsed.has(id);
      const parentsLaid: Laid[] = [];
      let x: number;
      if (n.parents.length === 2 && !isCollapsed) {
        const a = place(n.parents[0], id + ".0", depth + 1, n.gender_a);
        const b = place(n.parents[1], id + ".1", depth + 1, n.gender_b);
        parentsLaid.push(a, b);
        x = (a.x + b.x) / 2;
      } else {
        x = nextLeaf * (NODE_W + GAP_X);
        nextLeaf += 1;
      }
      const laid: Laid = {
        node: n,
        id,
        x,
        y: 0, // filled below once maxDepth is known
        depth,
        gender,
        collapsed: isCollapsed,
        parentsLaid,
      };
      nodes.push(laid);
      return laid;
    }

    const root = place(route.root, "r", 0, null);
    for (const l of nodes) {
      l.y = (maxDepth - l.depth) * GAP_Y;
    }
    const width = nextLeaf * (NODE_W + GAP_X);
    const height = (maxDepth + 1) * GAP_Y + NODE_H;
    return { nodes, root, width, height };
  });

  function kind(l: Laid): string {
    if (l.depth === 0) return "target";
    if (l.node.owned === "wild") return "wild";
    if (l.node.owned !== null) return "owned";
    return "bred";
  }

  function toggle(l: Laid) {
    if (!layout) return;
    const next = new Set(collapsed);
    const species = l.node.species;

    if (next.has(l.id)) {
      for (const n of layout.nodes) {
        if (n.node.species === species) {
          next.delete(n.id);
        }
      }
    } else if (l.node.parents.length === 2) {
      for (const n of layout.nodes) {
        if (n.node.species === species && n.node.parents.length === 2) {
          next.add(n.id);
        }
      }
    }

    collapsed = next;
    // Highlight the chain from this node down to the target
    const chain = new Set<string>();
    let id = l.id;
    while (id.length > 0) {
      chain.add(id);
      const cut = id.lastIndexOf(".");
      if (cut < 0) break;
      id = id.slice(0, cut);
    }
    selectedChain = chain;
  }

  function edgePath(parent: Laid, child: Laid): string {
    const x1 = parent.x + NODE_W / 2;
    const y1 = parent.y + NODE_H;
    const x2 = child.x + NODE_W / 2;
    const y2 = child.y;
    const my = (y1 + y2) / 2;
    return `M ${x1} ${y1} C ${x1} ${my}, ${x2} ${my}, ${x2} ${y2 - 6}`;
  }

  function genderSymbol(g: Gender | null): string {
    return g === "Male" ? " ♂" : g === "Female" ? " ♀" : "";
  }

</script>

{#if !route || !layout}
  <p class="hint">
    Plan a route first (Route Planner tab), then open it here with “Show tree”.
  </p>
{:else}
  <div class="wrap">
    <div class="legend">
      <span class="key target">target</span>
      <span class="key bred">bred (intermediate)</span>
      <span class="key owned">owned</span>
      <span class="key wild">wild catch</span>
      <span class="tip">scroll to zoom · drag to pan · click a node to collapse/expand</span>
    </div>
    <svg bind:this={svgEl} role="img" aria-label="Breeding tree">
      <defs>
        <marker
          id="arrow"
          viewBox="0 0 10 10"
          refX="9"
          refY="5"
          markerWidth="7"
          markerHeight="7"
          orient="auto-start-reverse"
        >
          <path d="M 0 0 L 10 5 L 0 10 z" fill="#5b6372" />
        </marker>
      </defs>
      <g transform="translate(40, 30)">
        <g {transform}>
          {#each layout.nodes as l (l.id)}
            {#each l.parentsLaid as p (p.id)}
              <path
                class="edge"
                class:hot={selectedChain.has(p.id) && selectedChain.has(l.id)}
                d={edgePath(p, l)}
                marker-end="url(#arrow)"
              />
            {/each}
          {/each}
          {#each layout.nodes as l (l.id)}
            <g
              class="node {kind(l)}"
              class:hot={selectedChain.has(l.id)}
              transform="translate({l.x}, {l.y})"
              onclick={() => toggle(l)}
              onkeydown={(e) => e.key === "Enter" && toggle(l)}
              role="button"
              tabindex="0"
            >
              <rect width={NODE_W} height={NODE_H} rx="10" />
              {#if l.node.icon}
                <image
                  href={"/icons/" + l.node.icon}
                  x="7"
                  y="7"
                  width="34"
                  height="34"
                />
              {/if}
              <text x="48" y={l.node.owned || l.collapsed ? 20 : 29} class="name">
                {l.node.name}{genderSymbol(l.gender)}
              </text>
              {#if l.collapsed}
                <text x="48" y="38" class="sub">▸ collapsed</text>
              {:else if l.node.owned === "wild"}
                <text x="48" y="38" class="sub">wild catch</text>
              {:else if l.node.owned !== null}
                <text x="48" y="38" class="sub">{l.node.owned}</text>
              {/if}
              {#if l.node.all_passives.length > 0}
                {@const desiredSet = new Set(l.node.passives)}
                {@const chips = l.node.all_passives}
                <g class="passives-row">
                  {#each chips as p, i}
                    {@const x = 8 + i * 40}
                    {#if x + 38 <= NODE_W}
                      {@const clipId = `clip-${l.id}-${i}`}
                      <defs>
                        <clipPath id={clipId}>
                          <rect x={x + 1} y={NODE_H - 18} width={34} height={14} rx={2} />
                        </clipPath>
                      </defs>
                      <g>
                        <title>{p}</title>
                        <rect
                          x={x}
                          y={NODE_H - 18}
                          width={36}
                          height={14}
                          rx={3}
                          class="passive-chip"
                          class:desired={desiredSet.has(p)}
                        />
                        <text
                          x={x + 18}
                          y={NODE_H - 8}
                          clip-path={`url(#${clipId})`}
                          class="passive-label"
                          class:desired={desiredSet.has(p)}
                        >{p.length > 7 ? p.slice(0, 7) + '…' : p}</text>
                      </g>
                    {/if}
                  {/each}
                </g>
              {:else if l.node.covered_passives.length > 0}
                {@const chips = l.node.covered_passives}
                <g class="passives-row">
                  {#each chips as p, i}
                    {@const x = 8 + i * 40}
                    {#if x + 38 <= NODE_W}
                      {@const clipId = `clip-${l.id}-${i}`}
                      <defs>
                        <clipPath id={clipId}>
                          <rect x={x + 1} y={NODE_H - 18} width={34} height={14} rx={2} />
                        </clipPath>
                      </defs>
                      <g>
                        <title>{p}</title>
                        <rect
                          x={x}
                          y={NODE_H - 18}
                          width={36}
                          height={14}
                          rx={3}
                          class="passive-chip covered"
                        />
                        <text
                          x={x + 18}
                          y={NODE_H - 8}
                          clip-path={`url(#${clipId})`}
                          class="passive-label covered"
                        >{p.length > 7 ? p.slice(0, 7) + '…' : p}</text>
                      </g>
                    {/if}
                  {/each}
                </g>
              {/if}
            </g>
          {/each}
        </g>
      </g>
    </svg>
  </div>
{/if}

<style>
  .hint {
    padding: 3rem;
    text-align: center;
    color: var(--text-dim);
  }

  .wrap {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 64px);
  }

  .legend {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.6rem 1.2rem;
    font-size: 0.8rem;
    color: var(--text-dim);
  }

  .key::before {
    content: "";
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 3px;
    margin-right: 0.35rem;
  }

  .key.target::before {
    background: var(--accent);
  }

  .key.bred::before {
    background: transparent;
    border: 2px solid #3b82f6;
  }

  .key.owned::before {
    background: #22c55e;
  }

  .key.wild::before {
    background: #3b82f6;
  }

  .tip {
    margin-left: auto;
  }

  svg {
    flex: 1;
    width: 100%;
    background:
      radial-gradient(circle, #22262e 1px, transparent 1px) 0 0 / 26px 26px,
      var(--bg);
    cursor: grab;
  }

  svg:active {
    cursor: grabbing;
  }

  .edge {
    fill: none;
    stroke: #5b6372;
    stroke-width: 1.5;
  }

  .edge.hot {
    stroke: var(--accent);
    stroke-width: 2.5;
  }

  .node {
    cursor: pointer;
  }

  .node rect {
    fill: var(--bg-raised);
    stroke: var(--border);
    stroke-width: 1.5;
  }

  .node.bred rect {
    stroke: #3b82f6;
  }

  .node.owned rect {
    fill: rgba(34, 197, 94, 0.18);
    stroke: #22c55e;
  }

  .node.wild rect {
    fill: rgba(59, 130, 246, 0.15);
    stroke: #3b82f6;
  }

  .node.target rect {
    fill: rgba(245, 158, 11, 0.2);
    stroke: var(--accent);
    stroke-width: 2;
  }

  .node.hot rect {
    stroke-width: 3;
  }

  .name {
    fill: var(--text);
    font-size: 13px;
    font-weight: 600;
  }

  .sub {
    fill: var(--text-dim);
    font-size: 11px;
  }

  .passive-chip {
    fill: rgba(156, 163, 175, 0.12);
    stroke: #9ca3af;
    stroke-width: 0.5;
  }

  .passive-chip.desired {
    fill: rgba(34, 197, 94, 0.18);
    stroke: #22c55e;
    stroke-width: 1.2;
  }

  .passive-chip.covered {
    fill: rgba(34, 197, 94, 0.15);
    stroke: #22c55e;
    stroke-width: 0.8;
  }

  .passive-label {
    fill: #9ca3af;
    font-size: 8px;
    text-anchor: middle;
    dominant-baseline: middle;
    pointer-events: none;
  }

  .passive-label.desired {
    fill: #22c55e;
    font-weight: 600;
  }

  .passive-label.covered {
    fill: #22c55e;
    font-weight: 600;
  }
</style>

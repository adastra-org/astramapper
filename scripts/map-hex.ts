// YMAP load pressure per hex cell, from a `map_audit` report.
//
//   bun scripts/map-hex.ts <map-audit.json> [--size 250] [--at x,y ...] [--out map-hex.json]
//
// The client crashes with `MapDataLoadedNode Pool Full, Size == 1100` when too
// many YMAPs are loaded at one spot. A YMAP is loaded while the camera is
// inside its streaming extents, so for each cell centre this counts the
// server's YMAPs whose streaming box holds it. Vanilla overrides are counted
// apart: they replace a game YMAP, they do not add one. The game's own YMAPs
// are not in the report, so the count is what the server adds, not the total.
//
// It also proposes merges: YMAPs of the same map and LOD class whose
// centres share a cell, when the merged box stays under the audit's
// `hugeExtentM`. Read-only: it writes a JSON and a Markdown next to it.

import { hexCenter, hexKey, worldToHex, type Hex } from "../src/lib/world/hex";

export type Box = { min: [number, number]; max: [number, number] };

export type AuditYmap = {
  file: string;
  resource: string;
  name: string;
  streamed: boolean;
  vanillaOverride: boolean | null;
  streamingMin: [number, number, number];
  streamingMax: [number, number, number];
  entitiesMin: [number, number, number];
  entitiesMax: [number, number, number];
  entities: number;
  lods: Record<string, number>;
  mloInstances: number;
  issues: string[];
  problem: string | null;
};

type Audit = { rules: { hugeExtentM: number }; ymaps: AuditYmap[] };

export type CellLoad = {
  q: number;
  r: number;
  x: number;
  y: number;
  added: number;
  overrides: number;
  byMap: Record<string, number>;
};

export type MergeGroup = {
  /** Resource plus the first folder under `stream`: one mapping. */
  map: string;
  lodClass: "detail" | "coarse";
  q: number;
  r: number;
  files: string[];
  saved: number;
  spanM: number;
};

/** Playable area of GTA V, padded. */
const WORLD: Box = { min: [-4500, -4500], max: [4500, 8500] };

/** Where the client crashed (crash logs of 2026-11-12 to 2026-11-15). */
const CRASH_SPOTS: [string, number, number][] = [
  ["Rockford", -1023, -418],
  ["Sandy Shores", 1758, 3812],
  ["Grapeseed", 2208, 4811],
];

const box2 = (y: AuditYmap): Box => ({
  min: [y.streamingMin[0], y.streamingMin[1]],
  max: [y.streamingMax[0], y.streamingMax[1]],
});

const contains = (b: Box, x: number, y: number) =>
  x >= b.min[0] && x <= b.max[0] && y >= b.min[1] && y <= b.max[1];

const isDetail = (lod: string) => lod === "hd" || lod === "orphan-hd";

/** `[astra]/Mapeados` + `stream/Vegetacion/YMAP/x.ymap` -> `[astra]/Mapeados/Vegetacion`. */
export function mapOf(y: AuditYmap): string {
  const parts = y.file.split("/");
  const s = parts.findIndex((p) => p.toLowerCase() === "stream");
  const folder = s >= 0 && s + 2 < parts.length ? parts[s + 1] : undefined;
  return folder ? `${y.resource}/${folder}` : y.resource;
}

export function lodClass(y: AuditYmap): "detail" | "coarse" | "mixed" {
  const keys = Object.keys(y.lods);
  const detail = keys.some(isDetail);
  const coarse = keys.some((k) => !isDetail(k));
  if (detail && coarse) return "mixed";
  return coarse ? "coarse" : "detail";
}

/** Every cell whose centre falls inside `area`. */
export function cellsIn(area: Box, size: number): Hex[] {
  const seen = new Map<number, Hex>();
  const step = size / 2;
  for (let x = area.min[0]; x <= area.max[0]; x += step) {
    for (let y = area.min[1]; y <= area.max[1]; y += step) {
      const h = worldToHex(x, y, size);
      const [cx, cy] = hexCenter(h, size);
      if (contains(area, cx, cy)) seen.set(hexKey(h.q, h.r), h);
    }
  }
  return [...seen.values()];
}

const loaded = (ymaps: AuditYmap[]) =>
  ymaps.filter((y) => y.streamed && !y.problem && y.entities + y.mloInstances > 0);

export function loadAt(ymaps: AuditYmap[], h: Hex, size: number): CellLoad {
  const [x, y] = hexCenter(h, size);
  const cell: CellLoad = { q: h.q, r: h.r, x, y, added: 0, overrides: 0, byMap: {} };
  for (const ym of ymaps) {
    if (!contains(box2(ym), x, y)) continue;
    if (ym.vanillaOverride) {
      cell.overrides++;
    } else {
      cell.added++;
      cell.byMap[mapOf(ym)] = (cell.byMap[mapOf(ym)] ?? 0) + 1;
    }
  }
  return cell;
}

export function loadPerCell(ymaps: AuditYmap[], size: number): CellLoad[] {
  const live = loaded(ymaps);
  return cellsIn(WORLD, size).map((h) => loadAt(live, h, size));
}

export function proposeMerges(ymaps: AuditYmap[], size: number, maxSpan: number): MergeGroup[] {
  const groups = new Map<string, { g: MergeGroup; box: Box }>();
  for (const y of loaded(ymaps)) {
    const cls = lodClass(y);
    if (y.vanillaOverride || y.mloInstances > 0 || cls === "mixed") continue;
    const cx = (y.entitiesMin[0] + y.entitiesMax[0]) / 2;
    const cy = (y.entitiesMin[1] + y.entitiesMax[1]) / 2;
    const h = worldToHex(cx, cy, size);
    const key = `${mapOf(y)}|${cls}|${h.q},${h.r}`;
    const b = box2(y);
    const cur = groups.get(key);
    if (!cur) {
      groups.set(key, {
        g: { map: mapOf(y), lodClass: cls, q: h.q, r: h.r, files: [y.file], saved: 0, spanM: 0 },
        box: b,
      });
      continue;
    }
    cur.g.files.push(y.file);
    cur.box = {
      min: [Math.min(cur.box.min[0], b.min[0]), Math.min(cur.box.min[1], b.min[1])],
      max: [Math.max(cur.box.max[0], b.max[0]), Math.max(cur.box.max[1], b.max[1])],
    };
  }
  const out: MergeGroup[] = [];
  for (const { g, box } of groups.values()) {
    if (g.files.length < 2) continue;
    g.spanM = Math.round(Math.max(box.max[0] - box.min[0], box.max[1] - box.min[1]));
    // A coarse (LOD) YMAP is meant to be wide; only detail is held to the cap.
    if (g.lodClass === "detail" && g.spanM > maxSpan) continue;
    g.saved = g.files.length - 1;
    out.push(g);
  }
  return out.toSorted((a, b) => b.saved - a.saved);
}

function topMaps(byMap: Record<string, number>, n = 3): string {
  return Object.entries(byMap)
    .toSorted((a, b) => b[1] - a[1])
    .slice(0, n)
    .map(([r, c]) => `${r.split("/").pop()} ${c}`)
    .join(", ");
}

function markdown(cells: CellLoad[], spots: CellLoad[], merges: MergeGroup[], size: number): string {
  const hot = cells.toSorted((a, b) => b.added - a.added).slice(0, 15);
  const row = (c: CellLoad, label = "") =>
    `| ${label}${label ? " " : ""}(${Math.round(c.x)}, ${Math.round(c.y)}) | ${c.added} | ${c.overrides} | ${topMaps(c.byMap)} |`;
  const saved = merges.reduce((s, g) => s + g.saved, 0);
  return [
    `# YMAP load per ${size} m hex cell`,
    "",
    "YMAPs of the server loaded with the camera at the cell centre. `added` adds to the game's own; `overrides` replace one. The pool that crashes holds 1100 in all, game included.",
    "",
    "## Crash spots",
    "",
    "| Spot | Added | Overrides | Top maps |",
    "|---|---|---|---|",
    ...spots.map((c, i) => row(c, CRASH_SPOTS[i]?.[0] ?? "")),
    "",
    "## Hottest cells",
    "",
    "| Cell centre | Added | Overrides | Top maps |",
    "|---|---|---|---|",
    ...hot.map((c) => row(c)),
    "",
    `## Merge proposals (${merges.length} groups, ${saved} YMAPs fewer)`,
    "",
    "| Map | LOD | Files | Saved | Span m |",
    "|---|---|---|---|---|",
    ...merges.slice(0, 30).map((g) => `| ${g.map} | ${g.lodClass} | ${g.files.length} | ${g.saved} | ${g.spanM} |`),
    "",
  ].join("\n");
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const input = args[0];
  if (!input) {
    console.error("usage: bun scripts/map-hex.ts <map-audit.json> [--size 250] [--at x,y] [--out map-hex.json]");
    process.exit(1);
  }
  let size = 250;
  let outPath = "map-hex.json";
  const at: [string, number, number][] = [];
  for (let i = 1; i < args.length; i++) {
    const a = args[i];
    if (a === "--size") size = Number(args[++i]);
    else if (a === "--out") outPath = args[++i] ?? outPath;
    else if (a === "--at") {
      const [x, y] = (args[++i] ?? "").split(",").map(Number);
      at.push([`(${x}, ${y})`, x ?? 0, y ?? 0]);
    } else throw new Error(`unknown argument ${a}`);
  }
  if (at.length) CRASH_SPOTS.splice(0, CRASH_SPOTS.length, ...at);

  const audit = (await Bun.file(input).json()) as Audit;
  const cells = loadPerCell(audit.ymaps, size);
  const live = loaded(audit.ymaps);
  const spots = CRASH_SPOTS.map(([, x, y]) => loadAt(live, worldToHex(x, y, size), size));
  const merges = proposeMerges(audit.ymaps, size, audit.rules.hugeExtentM);

  await Bun.write(outPath, JSON.stringify({ size, spots, cells, merges }));
  const md = markdown(cells, spots, merges, size);
  await Bun.write(outPath.replace(/\.json$/, "") + ".md", md);
  console.log(md);
}

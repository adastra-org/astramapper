/**
 * Draws one frame of the hex view onto a 2D canvas, and reports what it drew.
 *
 * Kept out of the component so the component stays about state and events: this
 * is a pure function of the model, the view and where the camera is, and the
 * numbers it returns are the ones the panel prints.
 */
import {
  hexCenter,
  hexDisk,
  hexDistance,
  hexKey,
  hexStats,
  worldToHex,
} from "../../lib/world/hex";
import { adaptiveLeafAt, adaptiveWorkingSet } from "../../lib/world/hexAdaptive";
import type { HexModel } from "../../lib/world/hexModel";

export const WIDTH = 460;
export const HEIGHT = 360;

export type View = { cx: number; cy: number; scale: number };

/** What the stats block shows, whichever mode produced it. */
export type Summary = {
  readonly cells: number;
  readonly placements: number;
  readonly maxCount: number;
  readonly setCells: number;
  readonly setPlacements: number;
  readonly setLoaded: number;
  /** Adaptive: how far the budget reaches, in metres. */
  readonly reach: number | null;
};

type Point = readonly [number, number];
type Point3 = readonly [number, number, number];

export type Frame = { readonly summary: Summary | null; readonly hover: string };

export function drawHex(
  ctx: CanvasRenderingContext2D,
  dpr: number,
  model: HexModel,
  view: View,
  cam: Point3 | null,
  forward: Point3 | null,
  mouseWorld: Point | null,
): Frame {
  const { cx, cy, scale } = view;
  const sx = (x: number) => WIDTH / 2 + (x - cx) * scale;
  const sy = (y: number) => HEIGHT / 2 - (y - cy) * scale;

  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = "#0d1015";
  ctx.fillRect(0, 0, WIDTH, HEIGHT);

  /** A hexagon by its centre, pointy-top like `hexCorners`. */
  const path = (x: number, y: number, size: number) => {
    ctx.beginPath();
    for (let i = 0; i < 6; i += 1) {
      const angle = (Math.PI / 180) * (60 * i - 30);
      const px = sx(x + size * Math.cos(angle));
      const py = sy(y + size * Math.sin(angle));
      if (i === 0) ctx.moveTo(px, py);
      else ctx.lineTo(px, py);
    }
    ctx.closePath();
  };
  const visible = (x: number, y: number, size: number) => {
    const margin = size * scale * 1.2;
    const px = sx(x);
    const py = sy(y);
    return px > -margin && px < WIDTH + margin && py > -margin && py < HEIGHT + margin;
  };
  const body = (
    x: number,
    y: number,
    size: number,
    count: number,
    loaded: number,
    logMax: number,
  ) => {
    if (!visible(x, y, size)) return;
    path(x, y, size);
    ctx.fillStyle = `rgba(224, 164, 88, ${0.1 + 0.6 * (Math.log1p(count) / logMax)})`;
    ctx.fill();
    ctx.strokeStyle = "rgba(255,255,255,0.07)";
    ctx.lineWidth = 1;
    ctx.stroke();
    // What is resident: a smaller teal hexagon, by how much of the cell.
    if (loaded > 0) {
      path(x, y, size * 0.55);
      ctx.fillStyle = `rgba(92, 200, 176, ${0.25 + 0.65 * Math.min(1, loaded / count)})`;
      ctx.fill();
    }
  };
  const ringStroke = (shade: number, lineWidth: number) => {
    ctx.strokeStyle = `hsl(35 ${shade === 0 ? 90 : 60}% ${Math.max(35, 82 - shade * 40)}%)`;
    ctx.lineWidth = lineWidth;
    ctx.stroke();
  };

  let frame: Frame = { summary: null, hover: "" };
  if (model.uniform) frame = drawUniform();
  else if (model.adaptive) frame = drawAdaptive();
  drawCamera();
  return frame;

  function drawUniform(): Frame {
    const cells = model.uniform!;
    const s = model.settings;
    let maxCount = 1;
    for (const cell of cells.values()) if (cell.count > maxCount) maxCount = cell.count;
    const logMax = Math.log1p(maxCount);
    for (const cell of cells.values()) {
      if (cell.count === 0) continue;
      const [x, y] = hexCenter(cell, s.size);
      body(x, y, s.size, cell.count, cell.loaded, logMax);
    }

    let summary: Summary | null = null;
    const center = cam ? worldToHex(cam[0], cam[1], s.size) : null;
    if (cam && center) {
      // The working set: the disk of rings around the camera's cell.
      for (const hex of hexDisk(center, s.radius)) {
        const [x, y] = hexCenter(hex, s.size);
        if (!visible(x, y, s.size)) continue;
        const ring = hexDistance(center, hex);
        path(x, y, s.size);
        ctx.fillStyle = `hsla(35, 80%, 60%, ${ring === 0 ? 0.22 : 0.05})`;
        ctx.fill();
        ringStroke(ring / Math.max(1, s.radius), ring === 0 ? 2 : 1.25);
      }
      const stats = hexStats(cells, center, s.radius);
      summary = {
        cells: stats.cells,
        placements: stats.placements,
        maxCount: stats.maxCount,
        setCells: stats.cellsInDisk,
        setPlacements: stats.placementsInDisk,
        setLoaded: stats.loadedInDisk,
        reach: null,
      };
    }

    let hover = "";
    if (mouseWorld) {
      const hex = worldToHex(mouseWorld[0], mouseWorld[1], s.size);
      const cell = cells.get(hexKey(hex.q, hex.r));
      hover =
        `cell ${hex.q},${hex.r} · ${(cell?.count ?? 0).toLocaleString()} placements` +
        ` · ${cell?.loaded ?? 0} loaded` +
        (center ? ` · ring ${hexDistance(center, hex)}` : "");
    }
    return { summary, hover };
  }

  function drawAdaptive(): Frame {
    const grid = model.adaptive!;
    const s = model.settings;
    const leaves = grid.cells;
    let maxCount = 1;
    let placements = 0;
    for (const cell of leaves) {
      placements += cell.count;
      if (cell.count > maxCount) maxCount = cell.count;
    }
    const logMax = Math.log1p(maxCount);
    // Coarse first, so the small cells of a city sit on top of the large ones
    // around it.
    // oxlint-disable-next-line unicorn/no-array-sort
    const ordered = [...leaves].sort((a, b) => a.level - b.level);
    for (const cell of ordered) body(cell.x, cell.y, cell.size, cell.count, cell.loaded, logMax);

    let summary: Summary | null = null;
    if (cam) {
      const set = adaptiveWorkingSet(grid, cam[0], cam[1], s.budget);
      let loaded = 0;
      for (const cell of set.cells) {
        loaded += cell.loaded;
        if (!visible(cell.x, cell.y, cell.size)) continue;
        path(cell.x, cell.y, cell.size);
        ctx.fillStyle = "hsla(35, 80%, 60%, 0.07)";
        ctx.fill();
        ringStroke(
          Math.min(1, Math.hypot(cell.x - cam[0], cell.y - cam[1]) / Math.max(set.reach, 1)),
          1.25,
        );
      }
      summary = {
        cells: leaves.length,
        placements,
        maxCount,
        setCells: set.cells.size,
        setPlacements: set.placements,
        setLoaded: loaded,
        reach: set.reach,
      };
    }

    let hover = "";
    if (mouseWorld) {
      const leaf = adaptiveLeafAt(grid, mouseWorld[0], mouseWorld[1]);
      hover = leaf
        ? `${leaf.size * 2} m cell · level ${leaf.level} · ${leaf.count.toLocaleString()} placements · ${leaf.loaded} loaded`
        : "no placements here";
    }
    return { summary, hover };
  }

  function drawCamera(): void {
    if (!cam) return;
    ctx.save();
    ctx.translate(sx(cam[0]), sy(cam[1]));
    if (forward) ctx.rotate(Math.atan2(-forward[1], forward[0]));
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.moveTo(9, 0);
    ctx.lineTo(-6, 5);
    ctx.lineTo(-6, -5);
    ctx.closePath();
    ctx.fill();
    ctx.restore();
  }
}

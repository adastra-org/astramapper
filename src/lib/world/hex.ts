/**
 * Hexagonal cells over the GTA ground plane, for the zone-based streaming the
 * world view is moving towards.
 *
 * Axial coordinates `(q, r)`, pointy-top, `size` being the distance from a
 * cell's centre to a corner in metres. Pure maths and counting, no rendering:
 * `HexView` draws it, and the eventual backend index will agree with it.
 *
 * Distances are in the X/Y plane. Height does not pick a cell — a camera above
 * a building is still in that building's cell.
 */

export type Hex = { readonly q: number; readonly r: number };

const SQRT3 = Math.sqrt(3);

/** The cell holding a ground-plane point. */
export function worldToHex(x: number, y: number, size: number): Hex {
  const q = ((SQRT3 / 3) * x - y / 3) / size;
  const r = ((2 / 3) * y) / size;
  return roundHex(q, r);
}

/** Cube-coordinate rounding: round all three, then repair the worst offender. */
function roundHex(q: number, r: number): Hex {
  const s = -q - r;
  let rq = Math.round(q);
  let rr = Math.round(r);
  const rs = Math.round(s);
  const dq = Math.abs(rq - q);
  const dr = Math.abs(rr - r);
  const ds = Math.abs(rs - s);
  if (dq > dr && dq > ds) rq = -rr - rs;
  else if (dr > ds) rr = -rq - rs;
  // `+ 0` turns a -0 into 0, which matters to anyone comparing with `toEqual`.
  return { q: rq + 0, r: rr + 0 };
}

/** A cell's centre in metres. */
export function hexCenter(hex: Hex, size: number): [number, number] {
  return [size * SQRT3 * (hex.q + hex.r / 2), size * 1.5 * hex.r];
}

/** A cell's six corners in metres, counter-clockwise from the east-north-east one. */
export function hexCorners(hex: Hex, size: number): [number, number][] {
  const [cx, cy] = hexCenter(hex, size);
  const corners: [number, number][] = [];
  for (let i = 0; i < 6; i += 1) {
    const angle = (Math.PI / 180) * (60 * i - 30);
    corners.push([cx + size * Math.cos(angle), cy + size * Math.sin(angle)]);
  }
  return corners;
}

/** Steps between two cells: 0 is the same cell, 1 a neighbour. */
export function hexDistance(a: Hex, b: Hex): number {
  const dq = a.q - b.q;
  const dr = a.r - b.r;
  return (Math.abs(dq) + Math.abs(dr) + Math.abs(dq + dr)) / 2;
}

/** Every cell within `radius` steps of `center`, the centre included. */
export function hexDisk(center: Hex, radius: number): Hex[] {
  const cells: Hex[] = [];
  for (let dq = -radius; dq <= radius; dq += 1) {
    const from = Math.max(-radius, -dq - radius);
    const to = Math.min(radius, -dq + radius);
    for (let dr = from; dr <= to; dr += 1) {
      cells.push({ q: center.q + dq, r: center.r + dr });
    }
  }
  return cells;
}

/** How many cells a disk of this radius holds: 1, 7, 19, 37, 61 … */
export function hexDiskSize(radius: number): number {
  return 1 + 3 * radius * (radius + 1);
}

/** A number key for a cell, cheap enough to build once per placement. */
export function hexKey(q: number, r: number): number {
  return (q + 32768) * 65536 + (r + 32768);
}

export type HexCell = {
  readonly q: number;
  readonly r: number;
  /** Placements whose position falls in the cell. */
  count: number;
  /** Placements with real geometry resident right now. */
  loaded: number;
  /** Sum of the placements' heights, so a mean can be drawn without a second pass. */
  zSum: number;
};

type Positioned = { readonly position: readonly [number, number, number] };

/**
 * Count placements per cell. `field` picks which tally to add to, so one map
 * carries both what exists (`count`) and what is resident (`loaded`).
 */
export function binInto(
  cells: Map<number, HexCell>,
  items: Iterable<Positioned>,
  size: number,
  field: "count" | "loaded",
): void {
  for (const item of items) {
    const { q, r } = worldToHex(item.position[0], item.position[1], size);
    const key = hexKey(q, r);
    let cell = cells.get(key);
    if (!cell) {
      cell = { q, r, count: 0, loaded: 0, zSum: 0 };
      cells.set(key, cell);
    }
    cell[field] += 1;
    if (field === "count") cell.zSum += item.position[2];
  }
}

/** Average height of a cell's placements, for drawing on the ground it covers. */
export function cellMeanZ(cell: { readonly count: number; readonly zSum: number }): number {
  return cell.count > 0 ? cell.zSum / cell.count : 0;
}

/** Placements per cell for every placed map. */
export function binPlacements(
  entries: readonly { readonly placements: readonly Positioned[] }[],
  size: number,
): Map<number, HexCell> {
  const cells = new Map<number, HexCell>();
  for (const entry of entries) binInto(cells, entry.placements, size, "count");
  return cells;
}

export type HexStats = {
  /** Cells that hold at least one placement. */
  readonly cells: number;
  readonly placements: number;
  readonly maxCount: number;
  /** Cells with content inside the disk around the camera. */
  readonly cellsInDisk: number;
  /** Placements a disk-sized working set would hold, against `placements`. */
  readonly placementsInDisk: number;
  readonly loadedInDisk: number;
};

/** What a working set of `radius` rings around `center` would hold. */
export function hexStats(
  cells: ReadonlyMap<number, HexCell>,
  center: Hex,
  radius: number,
): HexStats {
  let placements = 0;
  let maxCount = 0;
  let withContent = 0;
  for (const cell of cells.values()) {
    if (cell.count === 0) continue;
    withContent += 1;
    placements += cell.count;
    if (cell.count > maxCount) maxCount = cell.count;
  }
  let cellsInDisk = 0;
  let placementsInDisk = 0;
  let loadedInDisk = 0;
  for (const hex of hexDisk(center, radius)) {
    const cell = cells.get(hexKey(hex.q, hex.r));
    if (!cell || cell.count === 0) continue;
    cellsInDisk += 1;
    placementsInDisk += cell.count;
    loadedInDisk += cell.loaded;
  }
  return { cells: withContent, placements, maxCount, cellsInDisk, placementsInDisk, loadedInDisk };
}

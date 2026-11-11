/**
 * The cells the frontend holds, and the per-map view the streamer scans.
 *
 * # Why per map
 *
 * An entity is addressed everywhere by `(mapKey, entityIndex)`: the geometry
 * store, an in-progress edit, a selection, a saved project. That identity is the
 * map's path and the entity's index in its file, and it has to survive a cell
 * leaving and returning. So a cell is *stored* as the backend sends it, one part
 * per map, and *read* as the maps it touches: every part of one map, across the
 * cells held, merged into one `PlacementColumns` under the map's own key.
 *
 * Merging is lazy and per map. Adding or dropping a cell marks only the maps in
 * it dirty, and `entries()` rebuilds only those.
 */
import { mergeColumns, PlacementColumns } from "./columns";
import type { DecodedCell } from "./cellData";

/**
 * What the streamer's scan needs of a map: its key and its placements, with a
 * streaming box so wide that nothing is ever out of range. A cell is in the
 * store because the camera is near it; the map's declared extents no longer say
 * anything the cell does not.
 */
export type CellEntry = {
  readonly key: string;
  readonly name: string;
  readonly extentsMin: readonly [number, number, number];
  readonly extentsMax: readonly [number, number, number];
  readonly placements: PlacementColumns;
};

const BIG = 1e9;
const EVERYWHERE_MIN: readonly [number, number, number] = [-BIG, -BIG, -BIG];
const EVERYWHERE_MAX: readonly [number, number, number] = [BIG, BIG, BIG];

/** One placement a removed cell held, so its geometry can be unloaded. */
export type DroppedRow = { readonly mapKey: string; readonly entityIndex: number };

type MapSlot = {
  readonly name: string;
  /** This map's part of every cell held, by cell key. */
  readonly parts: Map<string, PlacementColumns>;
  merged: CellEntry | null;
};

export class CellStore {
  private readonly cells = new Map<string, DecodedCell>();
  private readonly maps = new Map<string, MapSlot>();
  private cached: readonly CellEntry[] | null = null;
  private held = 0;

  /** How many cells are held. */
  get size(): number {
    return this.cells.size;
  }

  /** How many placements are held, across every cell. */
  get placements(): number {
    return this.held;
  }

  has(key: string): boolean {
    return this.cells.has(key);
  }

  keys(): string[] {
    return [...this.cells.keys()];
  }

  /** Hold a cell. One already held is replaced. */
  add(cell: DecodedCell): void {
    if (this.cells.has(cell.key)) this.remove(cell.key);
    this.cells.set(cell.key, cell);
    this.held += cell.count;
    for (const part of cell.parts) {
      let slot = this.maps.get(part.mapKey);
      if (!slot) {
        slot = { name: part.mapName, parts: new Map(), merged: null };
        this.maps.set(part.mapKey, slot);
      }
      slot.parts.set(cell.key, part.columns);
      slot.merged = null;
    }
    this.cached = null;
  }

  /** Drop a cell, returning the placements it held. */
  remove(key: string): DroppedRow[] {
    const cell = this.cells.get(key);
    if (!cell) return [];
    this.cells.delete(key);
    this.held -= cell.count;
    const dropped: DroppedRow[] = [];
    for (const part of cell.parts) {
      const cols = part.columns;
      for (let row = 0; row < cols.length; row += 1) {
        dropped.push({ mapKey: part.mapKey, entityIndex: cols.entityIndex(row) });
      }
      const slot = this.maps.get(part.mapKey);
      if (!slot) continue;
      slot.parts.delete(key);
      slot.merged = null;
      if (slot.parts.size === 0) this.maps.delete(part.mapKey);
    }
    this.cached = null;
    return dropped;
  }

  clear(): void {
    this.cells.clear();
    this.maps.clear();
    this.cached = null;
    this.held = 0;
  }

  /**
   * The maps held, each as one entry. The array's identity holds until a cell is
   * added or dropped, so a memo keyed on it does not rerun for nothing.
   */
  entries(): readonly CellEntry[] {
    if (this.cached) return this.cached;
    const out: CellEntry[] = [];
    for (const [key, slot] of this.maps) {
      if (!slot.merged) {
        slot.merged = {
          key,
          name: slot.name,
          extentsMin: EVERYWHERE_MIN,
          extentsMax: EVERYWHERE_MAX,
          placements: mergeColumns([...slot.parts.values()]),
        };
      }
      out.push(slot.merged);
    }
    this.cached = out;
    return out;
  }
}

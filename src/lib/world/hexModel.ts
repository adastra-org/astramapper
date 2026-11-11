/**
 * The hex view's data, shared by everything that draws it: the panel and the
 * overlay in the 3D scene read the same model, so they cannot disagree about
 * which cells exist or which the loader would hold.
 */
import { useMemo } from "react";

import type { GeometryLoad } from "../world";
import {
  binInto,
  binPlacements,
  cellMeanZ,
  hexDisk,
  hexDistance,
  hexKey,
  worldToHex,
  hexCenter,
  type HexCell,
} from "./hex";
import {
  adaptiveWorkingSet,
  buildAdaptive,
  tallyAdaptiveLoaded,
  type AdaptiveGrid,
} from "./hexAdaptive";

export type HexMode = "uniform" | "adaptive";

export type HexSettings = {
  readonly mode: HexMode;
  /** Uniform: the cell size, centre to corner, in metres. */
  readonly size: number;
  /** Uniform: how many rings around the camera's cell are held. */
  readonly radius: number;
  /** Adaptive: the finest cell size. Coarser levels double up from it. */
  readonly fine: number;
  /** Adaptive: placements a cell may hold before it is split finer. */
  readonly threshold: number;
  /** Adaptive: placements the working set may hold. */
  readonly budget: number;
};

export const DEFAULT_HEX_SETTINGS: HexSettings = {
  mode: "uniform",
  size: 250,
  radius: 4,
  fine: 100,
  threshold: 2000,
  budget: 50_000,
};

/** How many sizes the adaptive grid runs through, coarse to fine. */
const ADAPTIVE_LEVELS = 5;

export function adaptiveSizes(fine: number): number[] {
  return Array.from({ length: ADAPTIVE_LEVELS }, (_, i) => fine * 2 ** (ADAPTIVE_LEVELS - 1 - i));
}

export type HexModel = {
  readonly settings: HexSettings;
  /** Uniform cells, with the resident tally on them. */
  readonly uniform: ReadonlyMap<number, HexCell> | null;
  readonly adaptive: AdaptiveGrid | null;
};

type Placements = { readonly placements: readonly { readonly position: readonly [number, number, number] }[] };

/**
 * Build what the current mode needs, and only that: counting three quarters of
 * a million placements is a pass over the world, so the mode that is off does
 * not pay it and a change to the loaded set does not repeat it.
 */
export function useHexModel(
  entries: readonly Placements[],
  geometry: readonly GeometryLoad[],
  settings: HexSettings,
  enabled: boolean,
): HexModel {
  const { mode, size, fine, threshold } = settings;

  const uniformBase = useMemo(
    () => (enabled && mode === "uniform" ? binPlacements(entries, size) : null),
    [enabled, mode, entries, size],
  );
  const adaptiveBase = useMemo(() => {
    if (!enabled || mode !== "adaptive") return null;
    let total = 0;
    for (const entry of entries) total += entry.placements.length;
    const xs = new Float64Array(total);
    const ys = new Float64Array(total);
    const zs = new Float64Array(total);
    let at = 0;
    for (const entry of entries) {
      for (const placement of entry.placements) {
        xs[at] = placement.position[0];
        ys[at] = placement.position[1];
        zs[at] = placement.position[2];
        at += 1;
      }
    }
    return buildAdaptive(xs, ys, zs, adaptiveSizes(fine), threshold);
  }, [enabled, mode, entries, fine, threshold]);

  // The resident tally is recounted whenever the loaded set changes.
  const uniform = useMemo(() => {
    if (!uniformBase) return null;
    for (const cell of uniformBase.values()) cell.loaded = 0;
    binInto(
      uniformBase,
      geometry.map((load) => load.entity),
      size,
      "loaded",
    );
    return uniformBase;
  }, [uniformBase, geometry, size]);
  const adaptive = useMemo(() => {
    if (!adaptiveBase) return null;
    tallyAdaptiveLoaded(
      adaptiveBase,
      geometry.map((load) => load.entity.position),
    );
    return adaptiveBase;
  }, [adaptiveBase, geometry]);

  return useMemo(() => ({ settings, uniform, adaptive }), [settings, uniform, adaptive]);
}

/** One cell as the 3D overlay draws it. */
export type OverlayCell = {
  /** Centre in game coordinates, metres. */
  readonly x: number;
  readonly y: number;
  /** Ground height to draw it at. */
  readonly z: number;
  /** Centre-to-corner radius, metres. */
  readonly size: number;
  /** 0 at the camera, 1 at the edge of the working set. */
  readonly shade: number;
};

/**
 * The cells the loader would hold for a camera: the disk of rings in uniform
 * mode, the budget-limited nearest leaves in adaptive mode.
 */
export function overlayCells(
  model: HexModel,
  camGta: readonly [number, number, number],
): OverlayCell[] {
  const { settings } = model;
  if (model.uniform) {
    const center = worldToHex(camGta[0], camGta[1], settings.size);
    const here = model.uniform.get(hexKey(center.q, center.r));
    // A cell with no placements has no ground to follow; borrow the camera
    // cell's, which is the nearest thing known, and failing that the camera's
    // own height less a little.
    const fallbackZ = here && here.count > 0 ? cellMeanZ(here) : camGta[2] - 5;
    return hexDisk(center, settings.radius).map((hex) => {
      const cell = model.uniform!.get(hexKey(hex.q, hex.r));
      const [x, y] = hexCenter(hex, settings.size);
      return {
        x,
        y,
        z: cell && cell.count > 0 ? cellMeanZ(cell) : fallbackZ,
        size: settings.size,
        shade: hexDistance(center, hex) / Math.max(1, settings.radius),
      };
    });
  }
  if (model.adaptive) {
    const set = adaptiveWorkingSet(model.adaptive, camGta[0], camGta[1], settings.budget);
    const reach = Math.max(set.reach, 1);
    return [...set.cells].map((cell) => ({
      x: cell.x,
      y: cell.y,
      z: cell.z,
      size: cell.size,
      shade: Math.hypot(cell.x - camGta[0], cell.y - camGta[1]) / reach,
    }));
  }
  return [];
}

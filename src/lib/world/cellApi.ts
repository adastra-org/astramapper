/**
 * The `invoke` wrappers for the backend's world index. Kept apart from
 * `world.ts` because they need a Tauri host and everything else in
 * `src/lib/world/` does not; the decoding they hand to is tested on its own.
 */
import { invoke } from "@tauri-apps/api/core";

import * as perf from "../perf";
import {
  decodeCellBatch,
  decodeCellGrid,
  type CellGrid,
  type CellRef,
  type DecodedBatch,
} from "./cellData";

/**
 * Read a content group into the backend's index and return its partition.
 *
 * The first call reads every map of the group (from the compiled-map cache once
 * it has been filled), so it is the one slow call of a session.
 */
export async function worldIndexBuild(
  group: string,
  options: { readonly fine?: number; readonly threshold?: number } = {},
): Promise<CellGrid> {
  const buffer = await perf.measureAsync(
    "worldIndex build ipc",
    () =>
      invoke<ArrayBuffer>("world_index_build", {
        group,
        fine: options.fine ?? null,
        threshold: options.threshold ?? null,
      }),
    (b) => b.byteLength,
  );
  return perf.measure("worldIndex build decode", () => decodeCellGrid(buffer));
}

/**
 * The placements of some cells and the models they need, in one reply.
 * `known` names are not resolved again and their models are not sent.
 */
export async function worldIndexCells(
  cells: readonly CellRef[],
  known: readonly string[],
): Promise<DecodedBatch> {
  const buffer = await perf.measureAsync(
    "worldIndex cells ipc",
    () => invoke<ArrayBuffer>("world_index_cells", { cells, known }),
    (b) => b.byteLength,
  );
  return perf.measure("worldIndex cells decode", () => decodeCellBatch(buffer));
}

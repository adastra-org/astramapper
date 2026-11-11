/**
 * The app's one persistent model tier (#7): created on first use, opened per
 * install and GPU, and what the world view fetches models through.
 *
 * The logic is in `persistentModels.ts` and tested there against an in-memory
 * store and against fake IndexedDB; this file is the wiring that needs a
 * browser and a Tauri host.
 */
import { reportGpuCaps } from "../gpu";
import * as perf from "../perf";
import { decodeModel, worldModelBuffer, type WorldModel } from "../world";
import { IdbBlobStore } from "./idbBlobStore";
import { diskBudgetBytes, loadThroughTier, PersistentModelTier, scopeOf } from "./persistentModels";

let tier: PersistentModelTier | null = null;

function modelTier(): PersistentModelTier {
  tier ??= new PersistentModelTier(new IdbBlobStore(), Date.now, (cause) =>
    // A broken store costs speed and never correctness; say so once and carry on.
    console.warn("persistent model cache:", cause),
  );
  return tier;
}

async function quota(): Promise<number | null> {
  try {
    return (await navigator.storage?.estimate())?.quota ?? null;
  } catch {
    return null;
  }
}

/**
 * Open the tier for an install, once its GPU capabilities are on record. A game
 * with no stamp (the demo plaza) leaves it closed, which is the IPC path.
 */
export async function openModelTier(installStamp: string | null): Promise<void> {
  if (installStamp === null) return;
  const caps = await reportGpuCaps();
  await modelTier().open(scopeOf(installStamp, caps), diskBudgetBytes(await quota()));
}

/** A model from disk when this machine has seen it under this install, else over IPC. */
export async function loadModelPersisted(path: string): Promise<WorldModel> {
  const t = modelTier();
  const { model } = await loadThroughTier(path, t, worldModelBuffer, decodeModel);
  perf.gauge("model disk tier (MB)", Math.round(t.bytes / 1048576));
  perf.gauge("model disk tier hits", t.hits);
  perf.gauge("model disk tier misses", t.misses);
  return model;
}

/** Empty the persistent tier, for a "clear cache" button. */
export function clearModelTier(): Promise<void> {
  return modelTier().clear();
}

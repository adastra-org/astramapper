/**
 * What to recommend when the user opens a game folder: precompile everything,
 * only the base game, or nothing (#7).
 *
 * Pure, so the thresholds are tested rather than guessed at in a component.
 *
 * # What the numbers mean
 *
 * A bake fills the backend's compiled-model cache up to its budget and stops
 * there (`src-tauri/src/bake.rs`), so the most it can write is
 * `budget - already used`. That, plus a margin the disk must keep free, is what
 * the recommendation weighs. The bake's pool is `cores - 1`, so the core count
 * decides how long it holds the machine, not whether it fits.
 */
import type { HardwareInfo } from "./world";

const GB = 1024 * 1024 * 1024;

/** What the disk must still have free after a full bake. */
export const DISK_MARGIN_BYTES = 5 * GB;
/** Below this much room a bake is not worth starting. */
export const MIN_ROOM_BYTES = 1 * GB;
/** The core count from which a full bake is a reasonable thing to leave running. */
export const FULL_BAKE_MIN_CORES = 4;
/** A cache already holding this many artifacts has had its precompile. */
export const ALREADY_WARM_RECORDS = 500;

export type PlanChoice = "all" | "base" | "skip";

export type PlanGroup = { readonly id: string };

export type PlanInput = {
  readonly hardware: HardwareInfo;
  readonly cacheBudgetBytes: number;
  readonly cacheUsedBytes: number;
  readonly groups: readonly PlanGroup[];
};

export type Plan = {
  readonly recommended: PlanChoice;
  /** Why, in plain sentences, most important first. */
  readonly reasons: readonly string[];
  /** The most a bake can write: the cache budget less what it already holds. */
  readonly roomBytes: number;
  /** The content groups each choice bakes. `skip` bakes none. */
  readonly groupsFor: Readonly<Record<PlanChoice, readonly string[]>>;
};

/** The group id the base game is under, or the first group when there is none. */
export function baseGroup(groups: readonly PlanGroup[]): string | null {
  return groups.find((group) => group.id === "base")?.id ?? groups[0]?.id ?? null;
}

export function planBake(input: PlanInput): Plan {
  const { hardware, groups } = input;
  const room = Math.max(0, input.cacheBudgetBytes - input.cacheUsedBytes);
  const base = baseGroup(groups);
  const groupsFor: Plan["groupsFor"] = {
    all: groups.map((group) => group.id),
    base: base === null ? [] : [base],
    skip: [],
  };
  const reasons: string[] = [];

  const free = hardware.freeDiskBytes;
  const usable = free === null ? room : Math.min(room, Math.max(0, free - DISK_MARGIN_BYTES));

  if (groups.length === 0) {
    return { recommended: "skip", reasons: ["No content groups to precompile."], roomBytes: room, groupsFor };
  }
  if (usable < MIN_ROOM_BYTES) {
    reasons.push(
      free === null
        ? "The cache has no room left."
        : `Only ${gb(free)} free on the cache drive, and a precompile needs it to keep ${gb(DISK_MARGIN_BYTES)} spare.`,
    );
    reasons.push("Models compile the first time they are seen instead.");
    return { recommended: "skip", reasons, roomBytes: room, groupsFor };
  }

  const fitsEverything = free === null ? false : free - room >= DISK_MARGIN_BYTES;
  if (free === null) reasons.push("Free disk space could not be read, so the choice is cautious.");
  else reasons.push(`${gb(free)} free on the cache drive.`);
  reasons.push(`${hardware.cores} cores: the precompile uses ${Math.max(1, hardware.cores - 1)}.`);

  if (fitsEverything && hardware.cores >= FULL_BAKE_MIN_CORES && groups.length > 1) {
    reasons.push(`Everything fits: at most ${gb(room)} of the cache is written.`);
    return { recommended: "all", reasons, roomBytes: room, groupsFor };
  }
  if (hardware.cores < FULL_BAKE_MIN_CORES) {
    reasons.push("Few cores: precompiling everything would hold the machine for a long time.");
  } else if (!fitsEverything) {
    reasons.push("Not enough spare disk for everything, so only the base game.");
  }
  return {
    recommended: base === null ? "skip" : "base",
    reasons,
    roomBytes: room,
    groupsFor,
  };
}

/**
 * Whether to put the choice in front of the user for this install: once, and not
 * at all when the cache is already warm (a previous precompile or a lot of use).
 */
export function shouldOfferBake(answered: boolean, cacheRecords: number | null): boolean {
  if (answered) return false;
  return cacheRecords === null ? false : cacheRecords < ALREADY_WARM_RECORDS;
}

function gb(bytes: number): string {
  return `${(bytes / GB).toFixed(bytes >= 10 * GB ? 0 : 1)} GB`;
}

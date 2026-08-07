/**
 * Reading the placement envelope `src-tauri/src/placements.rs` writes.
 *
 * # Why the placements are not JSON
 *
 * `world_map` used to be a plain command, so every placed entity crossed as
 * JSON text. Over the base game — 4,601 maps, 763,716 placements — that is
 * 201.5 MB and 2.3s to serialize on the Rust side alone, more than the 2.0s it
 * takes to read the maps in the first place. Most of those bytes are the
 * thirteen field *names*, repeated once per entity.
 *
 * # What this side still pays
 *
 * The columns arrive as views over the response buffer, but `Placed` is an
 * object with tuple fields and the whole renderer reads it that way, so the
 * decode below materializes one object per placement. That is deliberate and it
 * is the remaining cost: the win here is deleting the 201.5 MB of text and the
 * parse over it, not the object graph, which only a columnar `Placed` would
 * remove and which would touch every reader in `World.tsx`.
 *
 * Section order is the contract with the encoder and is documented there. This
 * module names the indices once, at the top, rather than spelling them into the
 * decode.
 */
import { asFloat32, asInt32, asUint16, asUint32, Kind, type Envelope } from "../wire";
import type { Placed, WorldMap } from "../world";

/** Section indices, in the order `placements::encode` pushes them. */
const SECTION = {
  index: 0,
  nameIndex: 1,
  flags: 2,
  position: 3,
  rotation: 4,
  scaleXy: 5,
  scaleZ: 6,
  parentIndex: 7,
  lodLevel: 8,
  lodDist: 9,
  children: 10,
} as const;

/** Bit 0 of the flags column. */
const FLAG_IS_MLO = 1;
/** Bit 1 of the flags column. */
const FLAG_FROM_MLO = 2;

/** The metadata the encoder writes, before the columns are laid over it. */
type PlacementMeta = {
  readonly path: string;
  readonly name: string;
  readonly nameHash: number;
  readonly parent: string;
  readonly entityCount: number;
  readonly mloEntities: number;
  readonly mloExpanded: number;
  readonly extentsMin: readonly [number, number, number];
  readonly extentsMax: readonly [number, number, number];
  readonly count: number;
  readonly names: readonly string[];
  readonly nameHashes: readonly number[];
  readonly lodLevels: readonly string[];
};

/**
 * Decode one placement envelope into the map the renderer reads.
 *
 * Throws on a kind mismatch or a short column, for the same reason
 * `readEnvelope` throws: either means the two halves of the format have
 * diverged, which is a build bug rather than a condition a caller can handle.
 * An entity whose name or LOD index falls outside its table is the one thing
 * that degrades instead — it keeps a placeholder and stays in the list, because
 * dropping a placement would silently move every `parentIndex` after it.
 */
export function decodePlacements(envelope: Envelope): WorldMap {
  if (envelope.kind !== Kind.Placements) {
    throw new Error(`envelope kind ${envelope.kind} is not a placement list`);
  }
  const meta = envelope.meta as PlacementMeta;
  const count = meta.count;

  const column = (which: number): Uint8Array => {
    const section = envelope.section(which);
    if (!section) throw new Error(`the placement envelope has no section ${which}`);
    return section;
  };

  const indexes = asUint32(column(SECTION.index));
  const nameIndexes = asUint32(column(SECTION.nameIndex));
  const flags = column(SECTION.flags);
  const positions = asFloat32(column(SECTION.position));
  const rotations = asFloat32(column(SECTION.rotation));
  const scaleXy = asFloat32(column(SECTION.scaleXy));
  const scaleZ = asFloat32(column(SECTION.scaleZ));
  const parents = asInt32(column(SECTION.parentIndex));
  const lodLevels = asUint16(column(SECTION.lodLevel));
  const lodDists = asFloat32(column(SECTION.lodDist));
  const children = asUint32(column(SECTION.children));

  if (
    indexes.length < count ||
    nameIndexes.length < count ||
    flags.length < count ||
    positions.length < count * 3 ||
    rotations.length < count * 4 ||
    scaleXy.length < count ||
    scaleZ.length < count ||
    parents.length < count ||
    lodLevels.length < count ||
    lodDists.length < count ||
    children.length < count
  ) {
    throw new Error(`the placement envelope declares ${count} entities but a column is shorter`);
  }

  // Sized up front rather than grown: this runs 763,716 times for the base
  // game, and a dense array assigned by index beats a repeatedly reallocated one.
  const entities: Placed[] = Array.from({ length: count });
  for (let at = 0; at < count; at += 1) {
    const slot = nameIndexes[at]!;
    const flag = flags[at]!;
    const p = at * 3;
    const r = at * 4;
    entities[at] = {
      index: indexes[at]!,
      name: meta.names[slot] ?? "",
      nameHash: meta.nameHashes[slot] ?? 0,
      isMlo: (flag & FLAG_IS_MLO) !== 0,
      fromMlo: (flag & FLAG_FROM_MLO) !== 0,
      position: [positions[p]!, positions[p + 1]!, positions[p + 2]!],
      rotation: [rotations[r]!, rotations[r + 1]!, rotations[r + 2]!, rotations[r + 3]!],
      scaleXy: scaleXy[at]!,
      scaleZ: scaleZ[at]!,
      parentIndex: parents[at]!,
      lodLevel: meta.lodLevels[lodLevels[at]!] ?? "",
      lodDist: lodDists[at]!,
      children: children[at]!,
    };
  }

  return {
    path: meta.path,
    name: meta.name,
    nameHash: meta.nameHash,
    parent: meta.parent,
    entityCount: meta.entityCount,
    mloEntities: meta.mloEntities,
    mloExpanded: meta.mloExpanded,
    extentsMin: meta.extentsMin,
    extentsMax: meta.extentsMax,
    entities,
  };
}

/**
 * Hand-built envelopes for the cell tests, written from the layout documented
 * in `src-tauri/src/world_index.rs` and `crates/cw-core/src/wire.rs` rather than
 * from the decoders they exercise, so a decoder and an encoder that drifted
 * apart do not agree with each other by construction.
 */
import { ENVELOPE_VERSION, Kind } from "../wire";
import type { Asset, CellRef } from "./cellData";

const HEADER_BYTES = 64;
const SECTION_ALIGN = 16;

const alignUp = (value: number): number => Math.ceil(value / SECTION_ALIGN) * SECTION_ALIGN;

export function buildEnvelope(
  kind: number,
  meta: unknown,
  sections: readonly Uint8Array[],
): ArrayBuffer {
  const metaBytes = new TextEncoder().encode(JSON.stringify(meta));
  const tableOffset = HEADER_BYTES + metaBytes.length;
  const blobOffset = alignUp(tableOffset + sections.length * 8);

  const offsets: number[] = [];
  let cursor = blobOffset;
  for (const section of sections) {
    cursor = alignUp(cursor);
    offsets.push(cursor);
    cursor += section.length;
  }

  const buffer = new ArrayBuffer(Math.max(cursor, HEADER_BYTES));
  const view = new DataView(buffer);
  const bytes = new Uint8Array(buffer);
  bytes.set([0x41, 0x4d, 0x42, 0x30], 0);
  view.setUint16(4, ENVELOPE_VERSION, true);
  view.setUint16(6, kind, true);
  view.setUint32(12, sections.length, true);
  view.setUint32(16, HEADER_BYTES, true);
  view.setUint32(20, metaBytes.length, true);
  view.setUint32(24, tableOffset, true);
  view.setUint32(28, blobOffset, true);
  bytes.set(metaBytes, HEADER_BYTES);
  sections.forEach((section, index) => {
    view.setUint32(tableOffset + index * 8, offsets[index]!, true);
    view.setUint32(tableOffset + index * 8 + 4, section.length, true);
    bytes.set(section, offsets[index]!);
  });
  return buffer;
}

const bytesOf = (view: ArrayBufferView): Uint8Array =>
  new Uint8Array(view.buffer, view.byteOffset, view.byteLength);

export const u8 = (values: readonly number[]): Uint8Array => Uint8Array.from(values);
export const u16 = (values: readonly number[]): Uint8Array => bytesOf(Uint16Array.from(values));
export const u32 = (values: readonly number[]): Uint8Array => bytesOf(Uint32Array.from(values));
export const i32 = (values: readonly number[]): Uint8Array => bytesOf(Int32Array.from(values));
export const f32 = (values: readonly number[]): Uint8Array => bytesOf(Float32Array.from(values));

// ---- The partition ------------------------------------------------------

export type LeafSpec = {
  readonly level: number;
  readonly q: number;
  readonly r: number;
  readonly count: number;
  readonly z?: number;
};

export function gridBuffer(
  sizes: readonly number[],
  leaves: readonly LeafSpec[],
  splits: readonly CellRef[] = [],
  overrides: {
    readonly placements?: number;
    readonly terrain?: number;
    /** Zone clusters and each leaf's index into them; omitted, as an older backend. */
    readonly clusters?: readonly {
      readonly key: string;
      readonly name: string;
      readonly leaves: number;
      readonly placements: number;
      readonly models: number;
    }[];
    readonly leafCluster?: readonly number[];
  } = {},
): ArrayBuffer {
  const placements = leaves.reduce((sum, l) => sum + l.count, 0) + (overrides.terrain ?? 0);
  return buildEnvelope(
    Kind.Cells,
    {
      group: "base",
      sizes,
      threshold: 1000,
      maps: 3,
      failed: 0,
      placements: overrides.placements ?? placements,
      terrain: overrides.terrain ?? 0,
      leaves: leaves.length,
      splits: splits.length,
      ...(overrides.clusters ? { clusters: overrides.clusters } : {}),
    },
    [
      u8(leaves.map((l) => l.level)),
      i32(leaves.map((l) => l.q)),
      i32(leaves.map((l) => l.r)),
      u32(leaves.map((l) => l.count)),
      f32(leaves.map((l) => l.z ?? 0)),
      u8(splits.map((s) => s.level)),
      i32(splits.map((s) => s.q)),
      i32(splits.map((s) => s.r)),
      ...(overrides.leafCluster ? [u16(overrides.leafCluster)] : []),
    ],
  );
}

// ---- A batch ------------------------------------------------------------

export type RowSpec = {
  readonly map: number;
  readonly index: number;
  readonly name: number;
  readonly x: number;
  readonly y: number;
  readonly z?: number;
  readonly lod?: number;
  readonly lodDist?: number;
};

export type BatchSpec = {
  readonly cells: readonly (CellRef & { readonly start: number; readonly count: number })[];
  readonly rows: readonly RowSpec[];
  readonly maps: readonly { readonly path: string; readonly name: string }[];
  readonly names: readonly string[];
  /** Archetype class per name; none when absent. */
  readonly classes?: readonly number[];
  readonly lodLevels?: readonly string[];
  readonly assets?: readonly Asset[];
  /** Each model is a complete `Kind.Model` envelope, nested as one section. */
  readonly models?: readonly { readonly path: string; readonly envelope: ArrayBuffer }[];
};

export function batchBuffer(spec: BatchSpec): ArrayBuffer {
  const rows = spec.rows;
  const models = spec.models ?? [];
  const columns = [
    u32(rows.map((r) => r.index)),
    u32(rows.map((r) => r.name)),
    u8(rows.map(() => 0)),
    f32(rows.flatMap((r) => [r.x, r.y, r.z ?? 0])),
    f32(rows.flatMap(() => [0, 0, 0, 1])),
    f32(rows.map(() => 1)),
    f32(rows.map(() => 1)),
    i32(rows.map(() => -1)),
    u16(rows.map((r) => r.lod ?? 0)),
    f32(rows.map((r) => r.lodDist ?? 100)),
    u32(rows.map(() => 0)),
    u32(rows.map((r) => r.map)),
  ];
  return buildEnvelope(
    Kind.CellData,
    {
      cells: spec.cells,
      count: rows.length,
      maps: spec.maps,
      names: spec.names,
      nameHashes: spec.names.map((_, i) => i + 1),
      nameClasses: spec.names.map((_, i) => spec.classes?.[i] ?? 0),
      lodLevels: spec.lodLevels ?? ["HD"],
      assets: spec.assets ?? [],
      models: models.map((m, i) => ({ path: m.path, section: columns.length + i })),
    },
    [...columns, ...models.map((m) => new Uint8Array(m.envelope))],
  );
}

/** A minimal compiled model: no drawables, so it decodes with no sections. */
export function emptyModelEnvelope(path: string): ArrayBuffer {
  return buildEnvelope(Kind.Model, { path, emptyDrawables: 0, textures: [], drawables: [] }, [
    u8([1, 2, 3, 4]),
  ]);
}

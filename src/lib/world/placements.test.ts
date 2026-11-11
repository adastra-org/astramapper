import { describe, expect, test } from "bun:test";

import type { PlacementColumns } from "./columns";
import { decodePlacements } from "./placements";

/** Every row as a `Placed`, for assertions about the whole map. */
function rowsOf(columns: PlacementColumns) {
  return Array.from({ length: columns.length }, (_, at) => columns.at(at));
}
import { ENVELOPE_VERSION, Kind, readEnvelope } from "../wire";

/**
 * The frontend half of the placement format, pinned against a hand-built
 * envelope.
 *
 * Written from the layout documented in `src-tauri/src/placements.rs` rather
 * than from the decoder under test, for the same reason `wire.test.ts` is: a
 * builder that shared code with the reader would agree with it no matter what
 * the encoder does.
 */

const HEADER_BYTES = 64;
const SECTION_ALIGN = 16;

function alignUp(value: number): number {
  return Math.ceil(value / SECTION_ALIGN) * SECTION_ALIGN;
}

function buildEnvelope(
  kind: number,
  meta: string,
  sections: readonly Uint8Array[],
): ArrayBuffer {
  const metaBytes = new TextEncoder().encode(meta);
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

const bytesOf = (array: ArrayBufferView): Uint8Array =>
  new Uint8Array(array.buffer, array.byteOffset, array.byteLength);

type Row = {
  index: number;
  nameIndex: number;
  flags: number;
  position: [number, number, number];
  rotation: [number, number, number, number];
  scaleXy: number;
  scaleZ: number;
  parentIndex: number;
  lodLevel: number;
  lodDist: number;
  children: number;
};

function row(overrides: Partial<Row> = {}): Row {
  return {
    index: 0,
    nameIndex: 0,
    flags: 0,
    position: [0, 0, 0],
    rotation: [0, 0, 0, 1],
    scaleXy: 1,
    scaleZ: 1,
    parentIndex: -1,
    lodLevel: 0,
    lodDist: 100,
    children: 0,
    ...overrides,
  };
}

/** Lay rows out column by column, the way the encoder does. */
function columnsOf(rows: readonly Row[]): Uint8Array[] {
  return [
    bytesOf(Uint32Array.from(rows.map((r) => r.index))),
    bytesOf(Uint32Array.from(rows.map((r) => r.nameIndex))),
    Uint8Array.from(rows.map((r) => r.flags)),
    bytesOf(Float32Array.from(rows.flatMap((r) => r.position))),
    bytesOf(Float32Array.from(rows.flatMap((r) => r.rotation))),
    bytesOf(Float32Array.from(rows.map((r) => r.scaleXy))),
    bytesOf(Float32Array.from(rows.map((r) => r.scaleZ))),
    bytesOf(Int32Array.from(rows.map((r) => r.parentIndex))),
    bytesOf(Uint16Array.from(rows.map((r) => r.lodLevel))),
    bytesOf(Float32Array.from(rows.map((r) => r.lodDist))),
    bytesOf(Uint32Array.from(rows.map((r) => r.children))),
  ];
}

function envelopeOf(
  rows: readonly Row[],
  meta: Record<string, unknown> = {},
  kind: number = Kind.Placements,
): ArrayBuffer {
  return buildEnvelope(
    kind,
    JSON.stringify({
      path: "x64a.rpf\\a.ymap",
      name: "a",
      nameHash: 7,
      parent: "",
      entityCount: rows.length,
      mloEntities: 0,
      mloExpanded: 0,
      extentsMin: [-1, -2, -3],
      extentsMax: [1, 2, 3],
      count: rows.length,
      names: ["prop_a", "prop_b"],
      nameHashes: [11, 22],
      lodLevels: ["HD", "LOD"],
      ...meta,
    }),
    columnsOf(rows),
  );
}

describe("decodePlacements", () => {
  test("lays the columns back into one object per placement", () => {
    const map = decodePlacements(
      readEnvelope(
        envelopeOf([
          row({
            index: 4,
            nameIndex: 1,
            position: [10, 20, 30],
            rotation: [0.5, -0.5, 0.5, -0.5],
            scaleXy: 2,
            scaleZ: 3,
            parentIndex: 9,
            lodLevel: 1,
            lodDist: 250,
            children: 6,
          }),
        ]),
      ),
    );

    expect(map.entities.length).toBe(1);
    expect(map.entities.at(0)).toEqual({
      index: 4,
      name: "prop_b",
      nameHash: 22,
      isMlo: false,
      fromMlo: false,
      position: [10, 20, 30],
      rotation: [0.5, -0.5, 0.5, -0.5],
      scaleXy: 2,
      scaleZ: 3,
      parentIndex: 9,
      lodLevel: "LOD",
      lodDist: 250,
      children: 6,
    });
  });

  test("carries the map's own fields through the metadata", () => {
    const map = decodePlacements(readEnvelope(envelopeOf([row()], { mloExpanded: 12 })));

    expect(map.path).toBe("x64a.rpf\\a.ymap");
    expect(map.name).toBe("a");
    expect(map.nameHash).toBe(7);
    expect(map.extentsMin).toEqual([-1, -2, -3]);
    expect(map.extentsMax).toEqual([1, 2, 3]);
    expect(map.mloExpanded).toBe(12);
  });

  test("reads the two MLO bits independently", () => {
    const map = decodePlacements(
      readEnvelope(
        envelopeOf([
          row({ flags: 0b00 }),
          row({ flags: 0b01 }),
          row({ flags: 0b10 }),
          row({ flags: 0b11 }),
        ]),
      ),
    );

    expect(rowsOf(map.entities).map((e) => [e.isMlo, e.fromMlo])).toEqual([
      [false, false],
      [true, false],
      [false, true],
      [true, true],
    ]);
  });

  test("keeps a negative parent index negative", () => {
    const map = decodePlacements(
      readEnvelope(envelopeOf([row({ parentIndex: -1 }), row({ parentIndex: 3 })])),
    );

    expect(rowsOf(map.entities).map((e) => e.parentIndex)).toEqual([-1, 3]);
  });

  test("resolves repeated names through the table rather than repeating them", () => {
    const map = decodePlacements(
      readEnvelope(
        envelopeOf([row({ nameIndex: 0 }), row({ nameIndex: 1 }), row({ nameIndex: 0 })]),
      ),
    );

    expect(rowsOf(map.entities).map((e) => e.name)).toEqual(["prop_a", "prop_b", "prop_a"]);
    expect(rowsOf(map.entities).map((e) => e.nameHash)).toEqual([11, 22, 11]);
  });

  test("an empty map decodes to no entities rather than throwing", () => {
    const map = decodePlacements(readEnvelope(envelopeOf([])));

    expect(map.entities.length).toBe(0);
    expect(map.entityCount).toBe(0);
  });

  /**
   * A name index outside the table is a bug on the encoding side, but dropping
   * the placement would shift every `parentIndex` after it — so it degrades to
   * a placeholder and keeps its slot.
   */
  test("an out-of-range name keeps the placement in the list", () => {
    const map = decodePlacements(
      readEnvelope(envelopeOf([row({ nameIndex: 99, lodLevel: 99 })])),
    );

    expect(map.entities.length).toBe(1);
    expect(map.entities.at(0).name).toBe("");
    expect(map.entities.at(0).nameHash).toBe(0);
    expect(map.entities.at(0).lodLevel).toBe("");
  });

  test("refuses an envelope that is not a placement list", () => {
    expect(() => decodePlacements(readEnvelope(envelopeOf([row()], {}, Kind.Model)))).toThrow(
      /not a placement list/,
    );
  });

  test("refuses a column shorter than the declared count", () => {
    // One row of columns, but the metadata claims two.
    expect(() => decodePlacements(readEnvelope(envelopeOf([row()], { count: 2 })))).toThrow(
      /a column is shorter/,
    );
  });
});

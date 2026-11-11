import { describe, expect, test } from "bun:test";
import { ENVELOPE_VERSION, Kind, readEnvelope, splitMips } from "./wire";

/**
 * The frontend half of the wire format, pinned against a hand-built envelope.
 *
 * The fixture below is written from the layout documented in
 * `crates/cw-core/src/wire.rs`, not from the decoder it tests — that is the
 * whole point. If the Rust encoder changes its header, this builder stops
 * matching it and the round-trip stops meaning anything, so the builder is
 * deliberately literal about offsets rather than sharing constants with the
 * code under test.
 */

const HEADER_BYTES = 64;
const SECTION_ALIGN = 16;

function alignUp(value: number): number {
  return Math.ceil(value / SECTION_ALIGN) * SECTION_ALIGN;
}

/** Build an envelope the way `Envelope::finish` does. */
function buildEnvelope(
  kind: number,
  meta: string,
  sections: readonly (readonly number[])[],
  version = ENVELOPE_VERSION,
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

  bytes.set([0x41, 0x4d, 0x42, 0x30], 0); // 'AMB0'
  view.setUint16(4, version, true);
  view.setUint16(6, kind, true);
  view.setUint32(8, 0, true); // flags
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

describe("readEnvelope", () => {
  test("reads the kind, the metadata and every section", () => {
    const buffer = buildEnvelope(Kind.Model, '{"path":"a.ydr","textures":[]}', [
      [1, 2, 3],
      [9, 9, 9, 9],
    ]);
    const envelope = readEnvelope(buffer);

    expect(envelope.kind).toBe(Kind.Model);
    expect(envelope.meta).toEqual({ path: "a.ydr", textures: [] });
    expect(envelope.sectionCount).toBe(2);
    expect([...envelope.section(0)!]).toEqual([1, 2, 3]);
    expect([...envelope.section(1)!]).toEqual([9, 9, 9, 9]);
  });

  test("returns null past the last section rather than throwing", () => {
    const envelope = readEnvelope(buildEnvelope(Kind.Model, "{}", [[1]]));
    expect(envelope.section(1)).toBeNull();
    expect(envelope.section(-1)).toBeNull();
  });

  test("reads an envelope with no sections", () => {
    const envelope = readEnvelope(buildEnvelope(Kind.Paths, '{"area":42}', []));
    expect(envelope.sectionCount).toBe(0);
    expect(envelope.meta).toEqual({ area: 42 });
  });

  /**
   * The property the format's padding exists for. A section view has to be
   * usable as a `Float32Array` without a copy, and that constructor throws on an
   * offset the encoder did not align.
   */
  test("sections are aligned enough for a zero-copy Float32Array view", () => {
    // Exactly representable in f32, so this compares the view against the
    // values and not against the rounding.
    const floats = [1, -2.5, 0.5, 1024.125];
    const raw = [...new Uint8Array(new Float32Array(floats).buffer)];
    // A metadata length that is not a multiple of 16, so alignment has to do
    // real work rather than falling out of the header size.
    const buffer = buildEnvelope(Kind.Model, '{"n":1234567}', [[7], raw]);
    const envelope = readEnvelope(buffer);

    const section = envelope.section(1)!;
    expect(section.byteOffset % 4).toBe(0);
    const view = new Float32Array(
      section.buffer,
      section.byteOffset,
      section.byteLength / 4,
    );
    expect([...view]).toEqual(floats);
  });

  test("a section view aliases the response buffer rather than copying it", () => {
    const buffer = buildEnvelope(Kind.Textures, "{}", [[1, 2, 3, 4]]);
    const envelope = readEnvelope(buffer);
    expect(envelope.section(0)!.buffer).toBe(buffer);
  });

  test("refuses a buffer that is not an envelope", () => {
    expect(() => readEnvelope(new ArrayBuffer(8))).toThrow();
    const buffer = buildEnvelope(Kind.Model, "{}", []);
    new Uint8Array(buffer)[0] = 0x58;
    expect(() => readEnvelope(buffer)).toThrow();
  });

  test("refuses a version it does not know", () => {
    const buffer = buildEnvelope(Kind.Model, "{}", [], ENVELOPE_VERSION + 1);
    expect(() => readEnvelope(buffer)).toThrow(/version/);
  });

  test("refuses a section pointing past the buffer", () => {
    const buffer = buildEnvelope(Kind.Model, "{}", [[1, 2, 3, 4]]);
    const view = new DataView(buffer);
    const tableOffset = view.getUint32(24, true);
    view.setUint32(tableOffset + 4, 0xffff, true);
    expect(() => readEnvelope(buffer)).toThrow(/past its buffer/);
  });
});

describe("splitMips", () => {
  test("splits a chain into views over the section", () => {
    const section = new Uint8Array([1, 1, 1, 1, 2, 2, 3]);
    const levels = splitMips(section, [4, 2, 1]);
    expect(levels.map((level) => [...level])).toEqual([[1, 1, 1, 1], [2, 2], [3]]);
    expect(levels[0]!.buffer).toBe(section.buffer);
  });

  test("stops at the end of the section rather than reading past it", () => {
    const section = new Uint8Array([1, 1, 1, 1, 2, 2]);
    // A chain that claims a level the section does not hold.
    expect(splitMips(section, [4, 2, 4])).toHaveLength(2);
  });

  test("an empty chain yields nothing", () => {
    expect(splitMips(new Uint8Array([1, 2, 3]), [])).toEqual([]);
  });
});

describe("readEnvelope on a nested envelope", () => {
  test("reads an envelope out of the middle of a larger buffer, sections still aligned", () => {
    const inner = buildEnvelope(Kind.Model, '{"path":"in.ydr"}', [[7, 8, 9], [1, 2, 3, 4]]);
    const outer = buildEnvelope(Kind.Placements, "{}", [[0], [...new Uint8Array(inner)]]);
    const carrier = readEnvelope(outer);
    const slice = carrier.section(1)!;
    expect(slice.byteOffset % 16).toBe(0);

    const nested = readEnvelope(outer, slice.byteOffset, slice.byteLength);
    expect(nested.kind).toBe(Kind.Model);
    expect(nested.meta).toEqual({ path: "in.ydr" });
    expect([...nested.section(0)!]).toEqual([7, 8, 9]);
    expect([...nested.section(1)!]).toEqual([1, 2, 3, 4]);
    // The views are over the outer buffer, at 16-byte boundaries.
    expect(nested.section(1)!.buffer).toBe(outer);
    expect(nested.section(1)!.byteOffset % 16).toBe(0);
  });

  test("a nested range that is too short for a header is refused", () => {
    const outer = buildEnvelope(Kind.Model, "{}", [[1, 2, 3]]);
    expect(() => readEnvelope(outer, 0, 10)).toThrow();
  });
});

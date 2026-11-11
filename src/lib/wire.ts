/**
 * Reading the binary envelope `crates/cw-core/src/wire.rs` writes.
 *
 * The two halves are pinned against each other by `wire.test.ts`, which builds
 * an envelope by hand from the constants below and reads it back. There is no
 * shared language to put a single implementation in, so what is shared instead
 * is the layout, documented once on the Rust side and mirrored here.
 *
 * Nothing in this module copies a section. `sectionView` hands back a
 * `Uint8Array` over the response's own buffer, which is the entire reason the
 * format aligns its sections: a `Float32Array` view throws on a misaligned
 * offset, so the encoder pads and this side can take views for free.
 */

/** `AMB0`, the four bytes an envelope starts with. */
const MAGIC = 0x30424d41; // little-endian 'A','M','B','0'

/** The header layout this decoder understands. */
export const ENVELOPE_VERSION = 1;

const HEADER_BYTES = 64;
const SECTION_ENTRY_BYTES = 8;

/** What an envelope carries, mirroring `wire::Kind`. */
export const Kind = {
  Model: 1,
  Textures: 2,
  Collision: 3,
  Paths: 4,
  Placements: 5,
  Cells: 6,
  CellData: 7,
} as const;

export type KindValue = (typeof Kind)[keyof typeof Kind];

export type Envelope = {
  readonly kind: number;
  /** The parsed JSON metadata. Its schema belongs to whoever wrote the kind. */
  readonly meta: unknown;
  readonly sectionCount: number;
  /**
   * A view over one section, or `null` when the index is out of range. The view
   * shares the response buffer — do not hold one past the life of the data it
   * describes, and do not write through it.
   */
  readonly section: (index: number) => Uint8Array | null;
};

/**
 * Read an envelope out of an IPC response.
 *
 * `byteOffset` and `byteLength` read an envelope nested inside a larger buffer,
 * as a cell reply carries its models. The nested one must start on the 16-byte
 * boundary the outer envelope puts its sections on, which keeps every view
 * inside it aligned.
 *
 * Throws rather than returning an error value, deliberately: a malformed
 * envelope means the two halves of the wire format have diverged, which is a
 * bug in the build and not a condition any caller can sensibly recover from.
 */
export function readEnvelope(
  buffer: ArrayBuffer,
  byteOffset = 0,
  byteLength: number = buffer.byteLength - byteOffset,
): Envelope {
  if (byteLength < HEADER_BYTES) {
    throw new Error(`the envelope is ${byteLength} bytes, shorter than its header`);
  }
  const header = new DataView(buffer, byteOffset, byteLength);
  if (header.getUint32(0, true) !== MAGIC) {
    throw new Error("the envelope does not start with AMB0");
  }
  const version = header.getUint16(4, true);
  if (version !== ENVELOPE_VERSION) {
    throw new Error(`the envelope is version ${version}, not ${ENVELOPE_VERSION}`);
  }

  const kind = header.getUint16(6, true);
  const sectionCount = header.getUint32(12, true);
  const metaOffset = header.getUint32(16, true);
  const metaLength = header.getUint32(20, true);
  const tableOffset = header.getUint32(24, true);

  if (metaOffset + metaLength > byteLength) {
    throw new Error("the envelope's metadata runs past its buffer");
  }
  const metaText = new TextDecoder().decode(
    new Uint8Array(buffer, byteOffset + metaOffset, metaLength),
  );
  const meta: unknown = metaText.length > 0 ? JSON.parse(metaText) : null;

  const offsets = new Uint32Array(sectionCount);
  const lengths = new Uint32Array(sectionCount);
  for (let index = 0; index < sectionCount; index += 1) {
    const at = tableOffset + index * SECTION_ENTRY_BYTES;
    if (at + SECTION_ENTRY_BYTES > byteLength) {
      throw new Error("the envelope's section table runs past its buffer");
    }
    offsets[index] = header.getUint32(at, true);
    lengths[index] = header.getUint32(at + 4, true);
    if (offsets[index]! + lengths[index]! > byteLength) {
      throw new Error(`envelope section ${index} runs past its buffer`);
    }
  }

  return {
    kind,
    meta,
    sectionCount,
    section: (index) => {
      if (index < 0 || index >= sectionCount) return null;
      return new Uint8Array(buffer, byteOffset + offsets[index]!, lengths[index]);
    },
  };
}

/**
 * A typed view over a section, without copying it.
 *
 * The envelope aligns every section to 16 bytes precisely so these cannot
 * throw, but the check stays: a misaligned view is the failure mode this whole
 * format exists to prevent, and finding out through a `RangeError` from deep
 * inside a constructor is not how anyone wants to learn the encoder regressed.
 */
function viewAs<T>(
  section: Uint8Array,
  bytesPer: number,
  make: (buffer: ArrayBufferLike, offset: number, length: number) => T,
): T {
  if (section.byteOffset % bytesPer !== 0) {
    throw new Error(
      `a section at byte ${section.byteOffset} is not aligned for ${bytesPer}-byte reads`,
    );
  }
  return make(section.buffer, section.byteOffset, Math.floor(section.byteLength / bytesPer));
}

export function asFloat32(section: Uint8Array): Float32Array {
  return viewAs(section, 4, (b, o, n) => new Float32Array(b, o, n));
}

export function asUint16(section: Uint8Array): Uint16Array {
  return viewAs(section, 2, (b, o, n) => new Uint16Array(b, o, n));
}

export function asUint32(section: Uint8Array): Uint32Array {
  return viewAs(section, 4, (b, o, n) => new Uint32Array(b, o, n));
}

export function asInt32(section: Uint8Array): Int32Array {
  return viewAs(section, 4, (b, o, n) => new Int32Array(b, o, n));
}

/**
 * Split one section into the mip levels a texture's metadata declares.
 *
 * Every level is a view, not a copy, so a whole chain costs one allocation for
 * the array holding them. The lengths are trusted as far as the section's own
 * end and no further: a chain that runs long is truncated rather than throwing,
 * because a partial mip chain is still an uploadable texture and a missing one
 * is not.
 */
export function splitMips(
  section: Uint8Array,
  byteLengths: readonly number[],
): Uint8Array[] {
  const out: Uint8Array[] = [];
  let at = 0;
  for (const length of byteLengths) {
    if (at + length > section.byteLength) break;
    out.push(section.subarray(at, at + length));
    at += length;
  }
  return out;
}

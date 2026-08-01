import { describe, expect, test } from "bun:test";
import {
  baseName,
  decodeBase64,
  decodeFloats,
  decodeUints,
  formatDuration,
} from "./viewer";

/**
 * Only the pure decoders are covered here. The `invoke` wrappers need a Tauri
 * host to say anything, so the commands behind them are covered by the Rust
 * tests in `src-tauri/src/viewer.rs` and the drawable gate.
 */

/** Base64 of a byte array, the way the backend produces it. */
function encode(bytes: number[]): string {
  return btoa(String.fromCharCode(...bytes));
}

describe("decodeBase64", () => {
  test("round-trips arbitrary bytes, high ones included", () => {
    const bytes = [0x00, 0x7f, 0x80, 0xff, 0x52, 0x50, 0x46, 0x37];
    expect([...decodeBase64(encode(bytes))]).toEqual(bytes);
  });

  test("decodes an empty buffer", () => {
    expect(decodeBase64("")).toHaveLength(0);
  });
});

describe("decodeFloats", () => {
  test("reads little-endian f32 the way the backend writes them", () => {
    // 1.0, -2.5, 0.5 as little-endian f32.
    const bytes = [
      0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x20, 0xc0, 0x00, 0x00, 0x00, 0x3f,
    ];
    expect([...decodeFloats(encode(bytes))]).toEqual([1, -2.5, 0.5]);
  });

  test("drops a trailing partial value rather than throwing", () => {
    const bytes = [0x00, 0x00, 0x80, 0x3f, 0x01, 0x02];
    expect([...decodeFloats(encode(bytes))]).toEqual([1]);
  });
});

describe("decodeUints", () => {
  test("reads little-endian u32 indices", () => {
    const bytes = [1, 0, 0, 0, 2, 0, 0, 0, 0xff, 0xff, 0, 0];
    expect([...decodeUints(encode(bytes))]).toEqual([1, 2, 65535]);
  });
});

describe("formatDuration", () => {
  test("renders minutes and padded seconds", () => {
    expect(formatDuration(0)).toBe("0:00");
    expect(formatDuration(9)).toBe("0:09");
    expect(formatDuration(65)).toBe("1:05");
    expect(formatDuration(3600)).toBe("60:00");
  });

  test("rounds to the nearest second", () => {
    expect(formatDuration(59.6)).toBe("1:00");
  });

  test("does not render a stream with no sample rate as NaN", () => {
    expect(formatDuration(Number.NaN)).toBe("0:00");
    expect(formatDuration(-1)).toBe("0:00");
  });
});

describe("baseName", () => {
  test("takes the last segment of an archive path", () => {
    expect(baseName(String.raw`x64a.rpf\models\skydome.ydd`)).toBe("skydome.ydd");
  });

  test("leaves a bare name alone", () => {
    expect(baseName("x64a.rpf")).toBe("x64a.rpf");
  });
});

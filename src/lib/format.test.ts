import { describe, expect, test } from "bun:test";
import { formatBytes, formatHash, formatOffset, hexLines, HEX_ROW } from "./format";

describe("formatHash", () => {
  test("pads to eight uppercase hex digits", () => {
    expect(formatHash(0)).toBe("0x00000000");
    expect(formatHash(0x23d9d79e)).toBe("0x23D9D79E");
    expect(formatHash(0xca2e9442)).toBe("0xCA2E9442");
  });

  test("treats the high bit as unsigned", () => {
    // What an i32 round-trip would deliver for 0xCA2E9442.
    expect(formatHash(-902917054)).toBe("0xCA2E9442");
  });
});

describe("formatBytes", () => {
  test("keeps small sizes in bytes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
  });

  test("scales up and drops the decimal past ten", () => {
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(27_729_408)).toBe("26 MB");
    expect(formatBytes(2_196_000_000)).toBe("2.0 GB");
  });
});

describe("hexLines", () => {
  test("lays out a full row", () => {
    const bytes = new Uint8Array([
      0x52, 0x50, 0x46, 0x37, 0x90, 0x02, 0x00, 0x00, 0x70, 0x2e, 0x00, 0x00,
      0xff, 0xff, 0xef, 0x0f,
    ]);
    const [line] = hexLines(bytes, 0);
    expect(line?.hex).toBe("52 50 46 37 90 02 00 00 70 2e 00 00 ff ff ef 0f");
    expect(line?.ascii).toBe("RPF7....p.......");
    expect(line?.offset).toBe(0);
  });

  test("counts offsets from the page's place in the file", () => {
    const bytes = new Uint8Array(HEX_ROW * 2);
    const lines = hexLines(bytes, 0x1000);
    expect(lines.map((l) => l.offset)).toEqual([0x1000, 0x1010]);
  });

  test("pads a short final row so the ascii column stays put", () => {
    const [line] = hexLines(new Uint8Array([0x41, 0x42]), 0);
    expect(line?.hex).toHaveLength(HEX_ROW * 3 - 1);
    expect(line?.hex.trimEnd()).toBe("41 42");
    expect(line?.ascii).toBe("AB");
  });

  test("has no rows for an empty page", () => {
    expect(hexLines(new Uint8Array(), 0)).toEqual([]);
  });
});

describe("formatOffset", () => {
  test("pads to eight uppercase hex digits", () => {
    expect(formatOffset(0)).toBe("00000000");
    expect(formatOffset(0x1f4)).toBe("000001F4");
  });
});

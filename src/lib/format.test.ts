import { describe, expect, test } from "bun:test";
import { formatBytes, formatHash } from "./format";

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

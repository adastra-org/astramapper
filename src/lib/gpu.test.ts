import { describe, expect, test } from "bun:test";
import { capsFromNames } from "./gpu";

/**
 * Only the pure half. `probe` needs a canvas and `reportGpuCaps` needs a Tauri
 * host, so what is covered here is the decision they feed on: which reported
 * extension names mean which capability.
 */

describe("capsFromNames", () => {
  test("reads a modern desktop context", () => {
    expect(
      capsFromNames([
        "EXT_color_buffer_float",
        "WEBGL_compressed_texture_s3tc",
        "WEBGL_compressed_texture_s3tc_srgb",
        "EXT_texture_compression_rgtc",
        "EXT_texture_compression_bptc",
      ]),
    ).toEqual({ s3tc: true, rgtc: true, bptc: true });
  });

  test("reports nothing for a context with no compression at all", () => {
    expect(capsFromNames([])).toEqual({ s3tc: false, rgtc: false, bptc: false });
  });

  /** Each extension gates its own formats; one missing is not all missing. */
  test("reports each extension independently", () => {
    const caps = capsFromNames(["WEBGL_compressed_texture_s3tc"]);
    expect(caps).toEqual({ s3tc: true, rgtc: false, bptc: false });
  });

  test("accepts a vendor-prefixed name", () => {
    const caps = capsFromNames(["WEBKIT_WEBGL_compressed_texture_s3tc"]);
    expect(caps.s3tc).toBe(true);
  });

  /**
   * `WEBGL_compressed_texture_s3tc_srgb` is a different extension covering
   * different internal formats, and it does not imply the one we ask for.
   */
  test("does not read the srgb variant as the base extension", () => {
    const caps = capsFromNames(["WEBGL_compressed_texture_s3tc_srgb"]);
    expect(caps.s3tc).toBe(false);
  });
});

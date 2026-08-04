/**
 * What this machine's GL context can upload, and telling the backend about it.
 *
 * The backend ships textures as the BC blocks they are stored as, which is
 * 660x cheaper than decoding them to RGBA and re-encoding them as PNG (see
 * `src-tauri/src/textures.rs`). That only works for formats the context has an
 * extension for, so the two sides agree once, up front, and the backend falls
 * back to RGBA for anything this reports as missing.
 *
 * Probing is split from deciding so the decision is testable: `capsFromNames`
 * is pure, and everything touching a canvas is in `probe`.
 */
import { setGpuCapabilities, type GpuCaps } from "./world";

export type { GpuCaps };

/**
 * The three extensions the texture path cares about, mapped from the names
 * `getSupportedExtensions` reports.
 *
 * Vendor prefixes are matched too. They are long dead on desktop, but the cost
 * of accepting `WEBKIT_WEBGL_compressed_texture_s3tc` is one `endsWith` and the
 * cost of missing it is every texture on that machine decoding to RGBA.
 */
export function capsFromNames(supported: readonly string[]): GpuCaps {
  const has = (name: string): boolean =>
    supported.some((extension) => extension === name || extension.endsWith(`_${name}`));
  return {
    s3tc: has("WEBGL_compressed_texture_s3tc"),
    rgtc: has("EXT_texture_compression_rgtc"),
    bptc: has("EXT_texture_compression_bptc"),
  };
}

/**
 * Ask a throwaway WebGL2 context what it supports.
 *
 * Deliberately not the renderer's own context: the answer is a property of the
 * driver, not of one context, and probing separately means the handshake can
 * complete before the viewport is built rather than racing it. A machine with
 * no WebGL at all reports nothing supported, which is the safe answer — the
 * backend then sends pixels, and there is no context to upload them to anyway.
 */
export function probe(): GpuCaps {
  try {
    const canvas = document.createElement("canvas");
    const gl = canvas.getContext("webgl2");
    if (!gl) return { s3tc: false, rgtc: false, bptc: false };
    const caps = capsFromNames(gl.getSupportedExtensions() ?? []);
    gl.getExtension("WEBGL_lose_context")?.loseContext();
    return caps;
  } catch {
    return { s3tc: false, rgtc: false, bptc: false };
  }
}

let reported: Promise<GpuCaps> | null = null;

/**
 * Probe once and tell the backend, then hand the same promise to every later
 * caller.
 *
 * **Await this before the first `worldModel` call.** Until it resolves the
 * backend has no capabilities on record and decodes every texture to RGBA:
 * correct, four times the VRAM, and entirely avoidable.
 */
export function reportGpuCaps(): Promise<GpuCaps> {
  if (!reported) {
    const caps = probe();
    reported = setGpuCapabilities(caps)
      .then(() => caps)
      // A failed handshake is not fatal: the backend's default is the
      // conservative one, so this costs fidelity and never correctness.
      .catch(() => ({ s3tc: false, rgtc: false, bptc: false }));
  }
  return reported;
}

/**
 * How the World view draws a part, from the shader it carries.
 *
 * A part arrives with the Jenkins hash of its shader's `.sps` file name
 * (`WorldPart.shaderFile`) and the shader's render bucket (`renderBucket`).
 * The base name (`WorldPart.shader`) is not enough: `normal` is also
 * `normal_cutout`, `default` is also `cutout`, and one tree mixes them.
 *
 * What the reference viewer does with them (`ShaderManager.Enqueue`):
 *
 * - **Not drawn.** `trees_shadow_proxy`, `cpv_only` and `decal_shadow_only` are
 *   shadow casters and vertex-colour helpers: "should add this to a shadowproxies
 *   group.. but currently don't draw". Every tree LOD 0 carries a proxy hull and
 *   drawing it in the colour pass is a solid blob around the canopy.
 * - **Alpha tested.** The shaders of its `CutoutBatches` run `BasicPS.hlsl`'s
 *   `if ((IsDecal == 0) && (c.a <= 0.33)) discard;`: leaves, fences, grates are a
 *   diffuse texture whose alpha is coverage. Without the test they are solid cards.
 * - Everything else draws as before.
 *
 * Evidence and the survey of the install are in `docs/formats.md`
 * ("Shaders: cutout and proxies").
 */

/** `ShaderManager.cs`, region "cutout batches", plus the trees (listed there too). */
const CUTOUT_SHADER_FILES: readonly string[] = [
  "cutout",
  "cutout_um",
  "cutout_tnt",
  "cutout_fence",
  "cutout_fence_normal",
  "cutout_hard",
  "cutout_spec_tnt",
  "normal_cutout",
  "normal_cutout_tnt",
  "normal_cutout_um",
  "normal_spec_cutout",
  "normal_spec_cutout_tnt",
  "trees_lod",
  "trees",
  "trees_tnt",
  "trees_normal",
  "trees_normal_spec",
  "trees_normal_spec_tnt",
  "trees_normal_diffspec",
  "trees_normal_diffspec_tnt",
  // The reference draws these cloth and vehicle shaders through the same pixel
  // shader, so they take the same test.
  "cloth_spec_cutout",
  "cloth_normal_spec_cutout",
  "vehicle_cutout",
  "weapon_normal_spec_cutout_palette",
];

/** "Should add this to a shadowproxies group.. but currently don't draw". */
const HIDDEN_SHADER_FILES: readonly string[] = [
  "trees_shadow_proxy",
  "cpv_only",
  "decal_shadow_only",
];

/** The render bucket the stock files use for cutouts (0 solid, 1 alpha, 2 decal, 3 cutout). */
export const CUTOUT_BUCKET = 3;

/** The alpha reference of `BasicPS.hlsl`: texels at or under it are discarded. */
export const ALPHA_TEST_REF = 0.33;

/** One-at-a-time Jenkins hash, as `JenkHash.GenHash` computes it (ASCII, case folded). */
export function jenkinsHash(text: string): number {
  let h = 0;
  for (let i = 0; i < text.length; i += 1) {
    let c = text.charCodeAt(i);
    if (c >= 65 && c <= 90) c += 32;
    h = (h + c) >>> 0;
    h = (h + (h << 10)) >>> 0;
    h = (h ^ (h >>> 6)) >>> 0;
  }
  h = (h + (h << 3)) >>> 0;
  h = (h ^ (h >>> 11)) >>> 0;
  h = (h + (h << 15)) >>> 0;
  return h >>> 0;
}

const fileHashes = (names: readonly string[]): ReadonlySet<number> =>
  new Set(names.map((name) => jenkinsHash(`${name}.sps`)));

const CUTOUT_HASHES = fileHashes(CUTOUT_SHADER_FILES);
const HIDDEN_HASHES = fileHashes(HIDDEN_SHADER_FILES);

export type PartDraw = "hidden" | "cutout" | "opaque";

/**
 * How a part is drawn. Models cached before the wire carried the file hash and
 * the bucket have neither (both read as undefined) and draw as they always did.
 */
export function partDraw(shaderFile: number | undefined, renderBucket: number | undefined): PartDraw {
  if (shaderFile === undefined) return "opaque";
  const file = shaderFile >>> 0;
  if (HIDDEN_HASHES.has(file)) return "hidden";
  if (CUTOUT_HASHES.has(file) || renderBucket === CUTOUT_BUCKET) return "cutout";
  return "opaque";
}

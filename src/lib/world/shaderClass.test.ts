import { describe, expect, test } from "bun:test";
import { CUTOUT_BUCKET, jenkinsHash, partDraw } from "./shaderClass";

// Hashes of `<name>.sps` as the reference viewer's switch statements list them
// (`ShaderManager.cs`, `Renderable.cs`).
const REFERENCE: Readonly<Record<string, number>> = {
  trees_normal: 3192134330,
  trees_normal_spec: 1224713457,
  trees_normal_diffspec: 4265705004,
  trees_normal_diffspec_tnt: 2245870123,
  trees_normal_spec_tnt: 1229591973,
  trees_tnt: 3334613197,
  trees: 2322653400,
  trees_lod: 582493193,
  trees_shadow_proxy: 1695474112,
  cpv_only: 83630553,
  normal_cutout: 1264076685,
  cutout: 1530399584,
  cutout_fence: 2219447268,
  normal_spec: 346521853,
  default: 413996436,
};

describe("jenkinsHash", () => {
  for (const [name, hash] of Object.entries(REFERENCE)) {
    test(`${name}.sps`, () => expect(jenkinsHash(`${name}.sps`)).toBe(hash));
  }

  test("folds case", () => expect(jenkinsHash("Trees_Normal.SPS")).toBe(REFERENCE.trees_normal!));
});

describe("partDraw", () => {
  test("alpha tests the tree shaders and the cutout families", () => {
    for (const name of ["trees_normal_spec", "trees_normal_diffspec_tnt", "trees", "normal_cutout", "cutout"]) {
      expect(partDraw(REFERENCE[name], 3)).toBe("cutout");
    }
  });

  test("does not draw shadow proxies or vertex-colour helpers", () => {
    expect(partDraw(REFERENCE.trees_shadow_proxy, 3)).toBe("hidden");
    expect(partDraw(REFERENCE.cpv_only, 0)).toBe("hidden");
  });

  test("tells a cutout from an opaque shader with the same base name by the file", () => {
    expect(partDraw(REFERENCE.normal_cutout, 0)).toBe("cutout");
    expect(partDraw(REFERENCE.normal_spec, 0)).toBe("opaque");
    expect(partDraw(REFERENCE.default, 0)).toBe("opaque");
  });

  test("trusts the cutout bucket for a shader it does not list", () => {
    expect(partDraw(123456, CUTOUT_BUCKET)).toBe("cutout");
    expect(partDraw(123456, 0)).toBe("opaque");
  });

  test("a model cached before the wire carried the file draws as it did", () => {
    expect(partDraw(undefined, undefined)).toBe("opaque");
  });
});

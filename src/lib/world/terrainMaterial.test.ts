import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { applyTerrainBlend, setTerrainAttributes, TERRAIN_ATTRIBUTES } from "./terrainMaterial";

const SHADER_STUB = {
  uniforms: {} as Record<string, { value: unknown }>,
  vertexShader: "#include <common>\n#include <begin_vertex>\n",
  fragmentShader: "#include <common>\n#include <map_fragment>\n",
};

function compile(material: THREE.MeshStandardMaterial): typeof SHADER_STUB {
  const shader = { ...SHADER_STUB, uniforms: {} };
  material.onBeforeCompile(shader as unknown as THREE.WebGLProgramParametersWithUniforms, null as never);
  return shader;
}

describe("terrain blend", () => {
  test("puts the two vertex colours and texcoord1 on the geometry", () => {
    const geometry = new THREE.BufferGeometry();
    // Two vertices: colour0 then colour1, four bytes each.
    const bytes = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 128, 1, 2, 3, 4, 5, 6, 7, 8]);
    setTerrainAttributes(geometry, bytes, new Float32Array([0, 1, 2, 3]));
    const c0 = geometry.getAttribute(TERRAIN_ATTRIBUTES.colour0);
    const c1 = geometry.getAttribute(TERRAIN_ATTRIBUTES.colour1);
    expect(c0.count).toBe(2);
    expect([c0.getX(1), c0.getY(1)]).toEqual([1 / 255, 2 / 255]);
    expect(c1.getY(0)).toBe(1);
    expect(c1.getW(0)).toBeCloseTo(128 / 255);
    expect(geometry.getAttribute(TERRAIN_ATTRIBUTES.uv1).count).toBe(2);
  });

  test("the shader blends the four layers and only samples the mask when there is one", () => {
    const layer = new THREE.Texture();
    const withMask = new THREE.MeshStandardMaterial();
    applyTerrainBlend(withMask, { layers: [layer, layer, layer, layer], mask: layer });
    const masked = compile(withMask);
    expect(masked.fragmentShader).toContain("texture2D(terrainMask, vTerrainUv1)");
    expect(masked.fragmentShader).toContain("terrainLayer3");
    expect(masked.uniforms.terrainLayer0?.value).toBe(layer);
    expect(masked.vertexShader).toContain(`attribute vec4 ${TERRAIN_ATTRIBUTES.colour0}`);

    const plain = new THREE.MeshStandardMaterial();
    applyTerrainBlend(plain, { layers: [layer, null, null, null], mask: null });
    const vertexOnly = compile(plain);
    expect(vertexOnly.fragmentShader).not.toContain("texture2D(terrainMask");
    // A layer that did not resolve is a grey texture, not an unbound sampler.
    expect(vertexOnly.uniforms.terrainLayer1?.value).toBeInstanceOf(THREE.DataTexture);
    expect(withMask.customProgramCacheKey()).not.toBe(plain.customProgramCacheKey());
  });
});

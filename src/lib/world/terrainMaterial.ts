/**
 * The ground's material: four layer textures blended by vertex data.
 *
 * Ground in the base game is the `terrain_cb_w_4lyr` family of shaders. The
 * reference's `TerrainPS.hlsl` samples four layers with `TEXCOORD0` and blends
 * them with a weight `w`:
 *
 * ```text
 * w  = mask ? mix(mask(TEXCOORD1), COLOUR1, COLOUR0.a) : COLOUR1
 * t1 = mix(layer0, layer1, w.b)
 * t2 = mix(layer2, layer3, w.b)
 * c  = mix(t1, t2, w.g)
 * ```
 *
 * `mask` is the `lookupSampler` texture of the `_cm` variants. Without it the
 * weight is the second vertex colour alone (the reference computes
 * `m * (1 - a) + vc1 * a` with `m = vc1`, which is `vc1`). Lighting stays
 * three.js's: this replaces only where the diffuse colour comes from.
 *
 * What is left out on purpose: the normal maps, specular, parallax (`pxm`) and
 * the tint of the golf variants. Those make ground look better, not textured.
 */
import * as THREE from "three";

/** What a terrain part blends. Every texture may be missing. */
export type TerrainBlend = {
  readonly layers: readonly (THREE.Texture | null)[];
  readonly mask: THREE.Texture | null;
};

/** A mid grey for a layer whose texture did not resolve. */
const fallback = new THREE.DataTexture(
  new Uint8Array([128, 128, 128, 255]),
  1,
  1,
  THREE.RGBAFormat,
  THREE.UnsignedByteType,
);
fallback.needsUpdate = true;

/** The vertex attributes the blend reads, named as `buildTerrainAttributes` sets them. */
export const TERRAIN_ATTRIBUTES = {
  colour0: "terrainColour0",
  colour1: "terrainColour1",
  uv1: "terrainUv1",
} as const;

/**
 * Put the blend's vertex data on a geometry: `COLOUR0` and `COLOUR1` out of the
 * interleaved bytes the backend sends, and texcoord1 when it has one.
 *
 * The colours are views over the response with a stride of 8, which is how a
 * `BufferAttribute` over one shared buffer is spelled in three.js.
 */
export function setTerrainAttributes(
  geometry: THREE.BufferGeometry,
  colours: Uint8Array,
  uvs1: Float32Array | null,
): void {
  const buffer = new THREE.InterleavedBuffer(colours, 8);
  geometry.setAttribute(
    TERRAIN_ATTRIBUTES.colour0,
    new THREE.InterleavedBufferAttribute(buffer, 4, 0, true),
  );
  geometry.setAttribute(
    TERRAIN_ATTRIBUTES.colour1,
    new THREE.InterleavedBufferAttribute(buffer, 4, 4, true),
  );
  if (uvs1) geometry.setAttribute(TERRAIN_ATTRIBUTES.uv1, new THREE.BufferAttribute(uvs1, 2));
}

/**
 * Make `material` draw the blend. The textures are the caller's to hold and
 * release; this only points the shader at them.
 */
export function applyTerrainBlend(
  material: THREE.MeshStandardMaterial,
  blend: TerrainBlend,
): void {
  const layers = [0, 1, 2, 3].map((i) => blend.layers[i] ?? fallback);
  const mask = blend.mask ?? fallback;
  const hasMask = blend.mask !== null;
  material.color.set(0xffffff);
  material.customProgramCacheKey = () => `terrain-blend:${hasMask ? "mask" : "vertex"}`;
  material.onBeforeCompile = (shader) => {
    shader.uniforms.terrainLayer0 = { value: layers[0] };
    shader.uniforms.terrainLayer1 = { value: layers[1] };
    shader.uniforms.terrainLayer2 = { value: layers[2] };
    shader.uniforms.terrainLayer3 = { value: layers[3] };
    shader.uniforms.terrainMask = { value: mask };
    shader.vertexShader = shader.vertexShader
      .replace(
        "#include <common>",
        `#include <common>
attribute vec4 ${TERRAIN_ATTRIBUTES.colour0};
attribute vec4 ${TERRAIN_ATTRIBUTES.colour1};
attribute vec2 ${TERRAIN_ATTRIBUTES.uv1};
varying vec4 vTerrainColour0;
varying vec4 vTerrainColour1;
varying vec2 vTerrainUv0;
varying vec2 vTerrainUv1;`,
      )
      .replace(
        "#include <begin_vertex>",
        `#include <begin_vertex>
vTerrainColour0 = ${TERRAIN_ATTRIBUTES.colour0};
vTerrainColour1 = ${TERRAIN_ATTRIBUTES.colour1};
vTerrainUv0 = uv;
vTerrainUv1 = ${TERRAIN_ATTRIBUTES.uv1};`,
      );
    shader.fragmentShader = shader.fragmentShader
      .replace(
        "#include <common>",
        `#include <common>
uniform sampler2D terrainLayer0;
uniform sampler2D terrainLayer1;
uniform sampler2D terrainLayer2;
uniform sampler2D terrainLayer3;
uniform sampler2D terrainMask;
varying vec4 vTerrainColour0;
varying vec4 vTerrainColour1;
varying vec2 vTerrainUv0;
varying vec2 vTerrainUv1;`,
      )
      .replace(
        "#include <map_fragment>",
        `vec4 terrainWeight = vTerrainColour1;
${
  hasMask
    ? "terrainWeight = mix(texture2D(terrainMask, vTerrainUv1), vTerrainColour1, vTerrainColour0.a);"
    : ""
}
vec4 terrainA = mix(texture2D(terrainLayer0, vTerrainUv0), texture2D(terrainLayer1, vTerrainUv0), terrainWeight.b);
vec4 terrainB = mix(texture2D(terrainLayer2, vTerrainUv0), texture2D(terrainLayer3, vTerrainUv0), terrainWeight.b);
diffuseColor.rgb *= mix(terrainA, terrainB, terrainWeight.g).rgb;`,
      );
  };
  material.needsUpdate = true;
}

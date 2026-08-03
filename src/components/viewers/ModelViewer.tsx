import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { Box, CloudSun, Grid3x3, Layers, Loader2, Palette, Sun } from "lucide-react";
import {
  decodeBase64,
  decodeFloats,
  decodeUints,
  readModel,
  type MeshBundle,
  type MeshPart,
  type ModelPreview,
} from "../../lib/viewer";
import { formatHash } from "../../lib/format";

/**
 * Preview a drawable in three.js.
 *
 * Everything here uploads buffers the Rust side already decoded — see
 * `crates/cw-core/src/mesh.rs`. No vertex declaration, no stride and no
 * archive byte is interpreted in the WebView.
 *
 * Textures cross as decoded PNGs (`ModelPreview.textures`) resolved from the
 * drawables' embedded dictionaries or sibling `.ytd` files; a part whose
 * texture hash has no resolution falls back to the flat material.
 *
 * three.js is the documented phase 2 choice for Explorer previews;
 * `docs/architecture.md` leaves the wgpu-versus-three decision to the phase 3
 * world renderer, which is a different problem with different constraints.
 */
export function ModelViewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<ModelPreview | null>(null);
  const [selected, setSelected] = useState(0);
  const [lod, setLod] = useState(0);
  const [wireframe, setWireframe] = useState(false);
  const [textures, setTextures] = useState(true);
  const [lighting, setLighting] = useState<"lit" | "unlit">("lit");
  const [light, setLight] = useState(1);
  const [shadows, setShadows] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setSelected(0);
    setLod(0);
    readModel()
      .then((model) => {
        if (cancelled) return;
        setPreview(model);
        // Open on something that draws. A dictionary's first entry is often a
        // placeholder with no geometry, and landing on it looks like a failure.
        setSelected(model.drawables.find((d) => d.lods.length > 0)?.index ?? 0);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (error) {
    return <p className="p-4 text-xs text-red-400">{error}</p>;
  }
  if (!preview) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Loader2 size={16} className="animate-spin text-ink-dim" />
      </div>
    );
  }

  const drawable = preview.drawables[selected] ?? preview.drawables[0] ?? null;
  const bundle = drawable?.lods[Math.min(lod, drawable.lods.length - 1)] ?? null;

  // The loaded part count for the status line: what LOD 0 shows.
  const full = drawable?.lods[0] ?? null;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-edge px-4 text-xs text-ink-dim">
        {preview.drawables.length > 1 && (
          <select
            value={selected}
            onChange={(e) => setSelected(Number(e.target.value))}
            className="max-w-56 rounded bg-surface-2 px-1.5 py-0.5 text-xs text-ink outline-none"
          >
            {preview.drawables.map((d) => (
              <option key={d.index} value={d.index}>
                {d.name || `drawable ${d.index}`}
                {d.lods.length === 0 ? " (empty)" : ""}
              </option>
            ))}
          </select>
        )}

        {drawable && drawable.lods.length > 1 && (
          <span className="flex items-center gap-1">
            <Layers size={13} strokeWidth={1.75} />
            <select
              value={lod}
              onChange={(e) => setLod(Number(e.target.value))}
              className="rounded bg-surface-2 px-1.5 py-0.5 text-xs text-ink outline-none"
            >
              {drawable.lods.map((l, i) => (
                <option key={l.lod} value={i}>
                  LOD {l.lod}
                  {l.triangleCount === 0 ? " (empty)" : ""}
                </option>
              ))}
            </select>
          </span>
        )}

        <button
          type="button"
          onClick={() => setWireframe((on) => !on)}
          className={`flex items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink ${
            wireframe ? "text-accent" : ""
          }`}
        >
          <Grid3x3 size={13} strokeWidth={1.75} />
          Wireframe
        </button>

        {preview.textures.length > 0 && (
          <button
            type="button"
            onClick={() => setTextures((on) => !on)}
            className={`flex items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink ${
              textures ? "text-accent" : ""
            }`}
          >
            <Palette size={13} strokeWidth={1.75} />
            Textures
          </button>
        )}

        <span className="flex items-center gap-1">
          <Sun size={13} strokeWidth={1.75} />
          <select
            value={lighting}
            onChange={(e) => setLighting(e.target.value as "lit" | "unlit")}
            className="rounded bg-surface-2 px-1.5 py-0.5 text-xs text-ink outline-none"
          >
            <option value="lit">Lit</option>
            <option value="unlit">Unlit</option>
          </select>
        </span>

        {lighting === "lit" && (
          <>
            <label className="flex items-center gap-1.5">
              <CloudSun size={13} strokeWidth={1.75} />
              <input
                type="range"
                min={0}
                max={2.5}
                step={0.05}
                value={light}
                onChange={(e) => setLight(Number(e.target.value))}
                className="w-24"
              />
              <span className="w-8 text-ink">{light.toFixed(2)}</span>
            </label>
            <button
              type="button"
              onClick={() => setShadows((on) => !on)}
              className={`flex items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink ${
                shadows ? "text-accent" : ""
              }`}
            >
              Shadows
            </button>
          </>
        )}

        {full && (
          <>
            <span className="flex items-center gap-1.5">
              <Box size={13} strokeWidth={1.75} />
              {full.parts.length} {full.parts.length === 1 ? "part" : "parts"},{" "}
              {full.triangleCount.toLocaleString()} triangles,{" "}
              {full.vertexCount.toLocaleString()} vertices
            </span>
            <span>{preview.textures.length} textures</span>
          </>
        )}
        {preview.emptyDrawables > 0 && (
          <span>{preview.emptyDrawables} without geometry</span>
        )}
      </div>

      {bundle ? (
        <Canvas
          bundle={bundle}
          wireframe={wireframe}
          textured={textures}
          textures={textures ? preview.textures : null}
          lighting={lighting}
          light={light}
          shadows={shadows}
        />
      ) : (
        <div className="flex min-h-0 flex-1 items-center justify-center text-sm text-ink-dim">
          {drawable?.name || "This drawable"} holds no renderable geometry.
        </div>
      )}

      {bundle && (
        <div className="shrink-0 overflow-x-auto border-t border-edge px-4 py-1.5 text-xs text-ink-dim">
          <span className="font-mono">
            {bundle.parts
              .map((p) => `${p.name} ${formatHash(p.shader)}`)
              .join("   ")}
          </span>
        </div>
      )}
    </div>
  );
}

type CanvasProps = {
  bundle: MeshBundle;
  wireframe: boolean;
  textured: boolean;
  textures: readonly {
    hash: number;
    name: string;
    width: number;
    height: number;
    png: string;
  }[] | null;
  lighting: "lit" | "unlit";
  light: number;
  shadows: boolean;
};

/** The texture map for one part, resolved to three.js textures. */
function resolvePartTextures(
  part: MeshPart,
  textures: CanvasProps["textures"],
): { diffuse: THREE.Texture | null; normal: THREE.Texture | null } {
  if (!textures) return { diffuse: null, normal: null };
  const byHash = new Map(textures.map((t) => [t.hash, t]));
  const diffuse = part.diffuse === null ? null : byHash.get(part.diffuse);
  const normal = part.normal === null ? null : byHash.get(part.normal);
  return {
    diffuse: diffuse ? textureFromPng(diffuse.png) : null,
    normal: normal ? normalTextureFromPng(normal.png) : null,
  };
}

const textureCache = new Map<string, THREE.Texture>();

/**
 * Decode one resolved texture once and keep it for the session.
 *
 * GTA V stores its UVs for Direct3D, where `(0,0)` is the top-left texel and
 * `v` grows downward. three.js uses WebGL's convention, where `(0,0)` is the
 * bottom-left, so the decoded PNG (file row 0 at the top) must NOT be re-flipped
 * on upload: `flipY = false` is what makes the model's `v = 0` land on the
 * texture's top row instead of mirroring it. Without it every part is rendered
 * vertically mirrored, which reads as a texture that does not line up.
 */
function textureFromPng(png: string): THREE.Texture {
  let cached = textureCache.get(png);
  if (cached) return cached;
  const texture = new THREE.Texture();
  texture.flipY = false;
  textureCache.set(png, texture);
  loadPngInto(png, texture, (image) => image);
  return texture;
}

/**
 * A bump map, decoded and reoriented for three.js.
 *
 * RAGE bump maps are left-handed (DirectX) tangent space: the reference shader
 * pushes the red channel along the tangent and the green along the bitangent
 * (`NormalMap` in `Common.hlsli`). three.js expects the right-handed OpenGL
 * convention, where the bitangent points the other way, so the green channel is
 * inverted here. Without it the detail reads inverted — bumps become dents.
 */
function normalTextureFromPng(png: string): THREE.Texture {
  const key = `normal:${png}`;
  let cached = textureCache.get(key);
  if (cached) return cached;
  const texture = new THREE.Texture();
  texture.flipY = false;
  textureCache.set(key, texture);
  loadPngInto(png, texture, invertNormalGreen);
  return texture;
}

/** Load a PNG into a texture asynchronously, transforming it on the way in. */
function loadPngInto(
  png: string,
  texture: THREE.Texture,
  transform: (image: HTMLImageElement) => HTMLImageElement | HTMLCanvasElement,
): void {
  const bytes = decodeBase64(png);
  const blob = new Blob([bytes as BlobPart], { type: "image/png" });
  const url = URL.createObjectURL(blob);
  const image = new Image();
  image.addEventListener("load", () => {
    texture.image = transform(image);
    texture.needsUpdate = true;
    URL.revokeObjectURL(url);
  });
  image.src = url;
}

/** Copy an image, flipping every green channel: `g' = 255 - g`. */
function invertNormalGreen(
  source: HTMLImageElement,
): HTMLImageElement | HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = source.naturalWidth || source.width;
  canvas.height = source.naturalHeight || source.height;
  const context = canvas.getContext("2d");
  if (!context) return source;
  context.drawImage(source, 0, 0);
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
  for (let i = 0; i < pixels.data.length; i += 4) {
    pixels.data[i + 1] = 255 - pixels.data[i + 1];
  }
  context.putImageData(pixels, 0, 0);
  return canvas;
}

/**
 * The WebGL surface. Split out so the scene is rebuilt when the bundle changes
 * and torn down exactly once, rather than on every toolbar re-render.
 */
function Canvas({
  bundle,
  wireframe,
  textured,
  textures,
  lighting,
  light,
  shadows,
}: CanvasProps): React.JSX.Element {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const container = host.current;
    if (!container) return;

    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.shadowMap.enabled = shadows;
    renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    container.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(50, 1, 0.01, 10_000);

    // Every light scales with the toolbar's intensity knob; MeshBasicMaterial
    // in Unlit mode ignores them all, which is what the mode is for.
    scene.add(new THREE.HemisphereLight(0xdfe8f5, 0x33383f, 1.1 * light));
    const key = new THREE.DirectionalLight(0xffffff, 2.4 * light);
    key.position.set(4, 8, 6);
    key.castShadow = shadows;
    scene.add(key);
    // A warm low fill so the shadowed side of a prop is not pitch black.
    const fill = new THREE.DirectionalLight(0xfff2dd, 0.5 * light);
    fill.position.set(-6, -2, -4);
    scene.add(fill);

    // GTA V is Z-up; three.js is Y-up. Rotating the whole model rather than
    // every buffer keeps the vertex data exactly as the file stores it.
    const model = new THREE.Group();
    model.rotation.x = -Math.PI / 2;
    scene.add(model);

    const geometries: THREE.BufferGeometry[] = [];
    for (const part of bundle.parts) {
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute(
        "position",
        new THREE.BufferAttribute(decodeFloats(part.positions), 3),
      );
      if (part.normals) {
        geometry.setAttribute(
          "normal",
          new THREE.BufferAttribute(decodeFloats(part.normals), 3),
        );
      }
      if (part.uvs) {
        geometry.setAttribute("uv", new THREE.BufferAttribute(decodeFloats(part.uvs), 2));
      }
      geometry.setIndex(new THREE.BufferAttribute(decodeUints(part.indices), 1));
      if (!part.normals) geometry.computeVertexNormals();
      geometries.push(geometry);

      const partMaterial = buildPartMaterial(part, {
        lighting,
        textured,
        textures,
        wireframe,
      });

      const mesh = new THREE.Mesh(geometry, partMaterial);
      mesh.castShadow = shadows;
      mesh.receiveShadow = shadows;
      model.add(mesh);
    }

    // Frame the model from its own geometry rather than from the declared
    // bounding box: for a skinned drawable the box is in a different space.
    const box = new THREE.Box3().setFromObject(model);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    const extent = Math.max(size.x, size.y, size.z, 0.001);

    // The shadow camera is a light's view box; it needs to just cover the
    // model, or the shadow is either wasted resolution or clipped to nothing.
    if (shadows) {
      const shadowHalf = extent;
      key.shadow.camera.left = -shadowHalf;
      key.shadow.camera.right = shadowHalf;
      key.shadow.camera.top = shadowHalf;
      key.shadow.camera.bottom = -shadowHalf;
      key.shadow.camera.near = 0.1;
      key.shadow.camera.far = extent * 20;
      key.shadow.mapSize.set(2048, 2048);
      key.shadow.camera.updateProjectionMatrix();
      key.target.position.copy(center);
    }

    const grid = new THREE.GridHelper(extent * 4, 20, 0x3a4250, 0x2a303a);
    grid.position.y = box.min.y;
    scene.add(grid);

    camera.position.set(
      center.x + extent * 1.4,
      center.y + extent * 0.9,
      center.z + extent * 1.4,
    );
    camera.near = extent / 500;
    camera.far = extent * 200;
    camera.updateProjectionMatrix();

    const controls = new OrbitControls(camera, renderer.domElement);
    controls.target.copy(center);
    controls.enableDamping = true;
    controls.update();

    let frame = 0;
    const tick = () => {
      frame = requestAnimationFrame(tick);
      controls.update();
      renderer.render(scene, camera);
    };
    tick();

    const resize = () => {
      const { clientWidth, clientHeight } = container;
      if (clientWidth === 0 || clientHeight === 0) return;
      renderer.setSize(clientWidth, clientHeight, false);
      camera.aspect = clientWidth / clientHeight;
      camera.updateProjectionMatrix();
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(container);

    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      controls.dispose();
      for (const geometry of geometries) geometry.dispose();
      grid.dispose();
      key.shadow.map?.dispose();
      renderer.dispose();
      container.removeChild(renderer.domElement);
    };
  }, [bundle, wireframe, textured, textures, lighting, light, shadows]);

  return <div ref={host} className="min-h-0 flex-1 overflow-hidden bg-surface-0" />;
}

/**
 * The material for one part.
 *
 * Lit uses a standard material with the diffuse and normal maps; Unlit uses a
 * basic material, which ignores every light — the diffuse map still applies, so
 * toggling Textures still works, but brightness is flat rather than lit.
 */
function buildPartMaterial(
  part: MeshPart,
  options: {
    lighting: "lit" | "unlit";
    textured: boolean;
    textures: CanvasProps["textures"];
    wireframe: boolean;
  },
): THREE.Material {
  const { lighting, textured, textures, wireframe } = options;
  const resolved = textured && textures ? resolvePartTextures(part, textures) : null;

  if (lighting === "unlit") {
    const material = new THREE.MeshBasicMaterial({
      color: resolved?.diffuse ? 0xffffff : 0x9aa6b4,
      side: THREE.DoubleSide,
    });
    material.wireframe = wireframe;
    if (resolved?.diffuse) material.map = resolved.diffuse;
    return material;
  }

  const material = new THREE.MeshStandardMaterial({
    color: 0x9aa6b4,
    roughness: 0.8,
    metalness: 0.05,
    side: THREE.DoubleSide,
  });
  material.wireframe = wireframe;
  if (resolved?.diffuse) {
    material.map = resolved.diffuse;
    material.color.set(0xffffff);
  }
  if (resolved?.normal) {
    material.normalMap = resolved.normal;
  }
  return material;
}

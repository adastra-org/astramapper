import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import {
  Box,
  CloudSun,
  ExternalLink,
  Grid3x3,
  Images,
  Layers,
  Loader2,
  Palette,
  RotateCcw,
  Sun,
  X,
} from "lucide-react";
import {
  baseName,
  decodeBase64,
  decodeFloats,
  decodeUints,
  readModel,
  type MeshBundle,
  type MeshPart,
  type ModelPreview,
  type ModelTexture,
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
 * texture hash has no resolution falls back to the flat material. The TXD panel
 * lists them with a per-texture toggle and, for any that came from a sibling
 * `.ytd`, a button that opens that dictionary.
 *
 * three.js is the documented phase 2 choice for Explorer previews;
 * `docs/architecture.md` leaves the wgpu-versus-three decision to the phase 3
 * world renderer, which is a different problem with different constraints.
 */
export function ModelViewer({
  path,
  onOpenPath,
}: {
  path: string;
  /** Open a sibling entry, e.g. the `.ytd` a texture was resolved from. */
  onOpenPath?: (path: string) => void;
}): React.JSX.Element {
  const [preview, setPreview] = useState<ModelPreview | null>(null);
  const [selected, setSelected] = useState(0);
  const [lod, setLod] = useState(0);
  const [wireframe, setWireframe] = useState(false);
  const [textures, setTextures] = useState(true);
  const [lighting, setLighting] = useState<"lit" | "unlit">("lit");
  const [light, setLight] = useState(1);
  const [shadows, setShadows] = useState(true);
  const [texturePanel, setTexturePanel] = useState(false);
  const [disabledTextures, setDisabledTextures] = useState<ReadonlySet<number>>(
    new Set(),
  );
  const [previewTexture, setPreviewTexture] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setSelected(0);
    setLod(0);
    setTexturePanel(false);
    setDisabledTextures(new Set());
    setPreviewTexture(null);
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

  const toggleTexture = (hash: number, on: boolean) => {
    setDisabledTextures((disabled) => {
      const next = new Set(disabled);
      if (on) {
        next.delete(hash);
      } else {
        next.add(hash);
      }
      return next;
    });
  };

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
          <>
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
            <button
              type="button"
              onClick={() => setTexturePanel((on) => !on)}
              title="The model's texture dictionary"
              className={`flex items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink ${
                texturePanel ? "text-accent" : ""
              }`}
            >
              <Images size={13} strokeWidth={1.75} />
              TXD
            </button>
          </>
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
            <button
              type="button"
              onClick={() => {
                setLight(1);
                setShadows(true);
              }}
              title="Reset the light to its defaults"
              className="flex items-center rounded px-1 py-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
            >
              <RotateCcw size={13} strokeWidth={1.75} />
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

      <div className="flex min-h-0 flex-1">
        <div className="min-h-0 min-w-0 flex-1">
          {bundle ? (
            <Canvas
              bundle={bundle}
              wireframe={wireframe}
              textured={textures}
              textures={textures ? preview.textures : null}
              disabledTextures={disabledTextures}
              lighting={lighting}
              light={light}
              shadows={shadows}
            />
          ) : (
            <div className="flex h-full items-center justify-center text-sm text-ink-dim">
              {drawable?.name || "This drawable"} holds no renderable geometry.
            </div>
          )}
        </div>

        {texturePanel && preview.textures.length > 0 && (
          <TexturePanel
            textures={preview.textures}
            disabled={disabledTextures}
            selected={previewTexture}
            onSelect={setPreviewTexture}
            onToggle={toggleTexture}
            onOpenYtd={onOpenPath}
            onClose={() => setTexturePanel(false)}
          />
        )}
      </div>

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
  disabledTextures: ReadonlySet<number>;
  lighting: "lit" | "unlit";
  light: number;
  shadows: boolean;
};

/** The resolved texture list with any disabled hashes dropped. */
function activeTextures(
  textures: CanvasProps["textures"],
  disabled: ReadonlySet<number>,
): CanvasProps["textures"] {
  if (!textures) return null;
  if (disabled.size === 0) return textures;
  return textures.filter((t) => !disabled.has(t.hash));
}

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

const EMPTY_DISABLED: ReadonlySet<number> = new Set();

type LightRig = {
  hemi: THREE.HemisphereLight;
  key: THREE.DirectionalLight;
  fill: THREE.DirectionalLight;
};

type PartMesh = {
  mesh: THREE.Mesh;
  part: MeshPart;
};

type PartMaterialOptions = {
  lighting: "lit" | "unlit";
  textured: boolean;
  textures: CanvasProps["textures"];
  wireframe: boolean;
  disabledTextures: ReadonlySet<number>;
};

/**
 * The WebGL surface.
 *
 * The scene is built once per bundle; everything the toolbar can change — light
 * intensity, shadows, materials, texture resolution — is applied imperatively
 * afterwards, so the camera and orbit controls are never rebuilt and the view
 * stays exactly where it was while a slider moves.
 */
function Canvas({
  bundle,
  wireframe,
  textured,
  textures,
  disabledTextures,
  lighting,
  light,
  shadows,
}: CanvasProps): React.JSX.Element {
  const host = useRef<HTMLDivElement>(null);
  const rig = useRef<LightRig | null>(null);
  const ground = useRef<THREE.Mesh | null>(null);
  const partMeshes = useRef<PartMesh[]>([]);
  const lightingMode = useRef<"lit" | "unlit">("lit");

  useEffect(() => {
    const container = host.current;
    if (!container) return;

    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.shadowMap.enabled = true;
    renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    container.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(50, 1, 0.01, 10_000);

    const hemi = new THREE.HemisphereLight(0xdfe8f5, 0x33383f, 1.1);
    scene.add(hemi);
    const key = new THREE.DirectionalLight(0xffffff, 2.4);
    key.castShadow = true;
    key.shadow.bias = -0.0005;
    scene.add(key);
    // A directional light aims at its target, and a target that is not in the
    // scene never updates its matrix — without this the light keeps pointing at
    // the origin and the shadow misses any model that is not centred there.
    scene.add(key.target);
    // A warm low fill so the shadowed side of a prop is not pitch black.
    const fill = new THREE.DirectionalLight(0xfff2dd, 0.5);
    scene.add(fill);

    // GTA V is Z-up; three.js is Y-up. Rotating the whole model rather than
    // every buffer keeps the vertex data exactly as the file stores it.
    const model = new THREE.Group();
    model.rotation.x = -Math.PI / 2;
    scene.add(model);

    const geometries: THREE.BufferGeometry[] = [];
    const meshes: PartMesh[] = [];
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

      const mesh = new THREE.Mesh(
        geometry,
        buildPartMaterial(part, {
          lighting: "lit",
          textured: false,
          textures: null,
          wireframe: false,
          disabledTextures: EMPTY_DISABLED,
        }),
      );
      mesh.castShadow = true;
      mesh.receiveShadow = true;
      model.add(mesh);
      meshes.push({ mesh, part });
    }
    partMeshes.current = meshes;

    // Frame the model from its own geometry rather than from the declared
    // bounding box: for a skinned drawable the box is in a different space.
    const box = new THREE.Box3().setFromObject(model);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    const extent = Math.max(size.x, size.y, size.z, 0.001);

    // Place the key light relative to the model, not the origin, and fit its
    // shadow frustum around the model so the shadow is neither wasted nor gone.
    const keyDir = new THREE.Vector3(4, 8, 6).normalize();
    key.position.copy(center).addScaledVector(keyDir, extent * 3);
    key.target.position.copy(center);
    const shadowHalf = extent * 1.5;
    key.shadow.camera.left = -shadowHalf;
    key.shadow.camera.right = shadowHalf;
    key.shadow.camera.top = shadowHalf;
    key.shadow.camera.bottom = -shadowHalf;
    key.shadow.camera.near = extent * 0.5;
    key.shadow.camera.far = extent * 10;
    key.shadow.mapSize.set(2048, 2048);
    key.shadow.camera.updateProjectionMatrix();

    // A shadow catcher under the model: self-shadowing on a prop is subtle
    // enough to read as "no shadows", so the toggle needs a surface to land on.
    const catcher = new THREE.Mesh(
      new THREE.PlaneGeometry(extent * 4, extent * 4),
      new THREE.ShadowMaterial({ opacity: 0.35 }),
    );
    catcher.rotation.x = -Math.PI / 2;
    catcher.position.y = box.min.y - extent * 0.01;
    catcher.receiveShadow = true;
    scene.add(catcher);
    ground.current = catcher;

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

    rig.current = { hemi, key, fill };
    lightingMode.current = "lit";

    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      controls.dispose();
      for (const geometry of geometries) geometry.dispose();
      grid.dispose();
      catcher.geometry.dispose();
      (catcher.material as THREE.ShadowMaterial).dispose();
      key.shadow.map?.dispose();
      renderer.dispose();
      container.removeChild(renderer.domElement);
      rig.current = null;
      ground.current = null;
      partMeshes.current = [];
    };
  }, [bundle]);

  // Apply whatever the toolbar controls. Runs after the scene is built and on
  // every change to those controls; nothing here touches the camera.
  useEffect(() => {
    const lights = rig.current;
    if (!lights) return;

    lights.hemi.intensity = 1.1 * light;
    lights.key.intensity = 2.4 * light;
    lights.fill.intensity = 0.5 * light;

    lights.key.castShadow = shadows;
    if (ground.current) ground.current.visible = shadows;
    for (const { mesh } of partMeshes.current) {
      mesh.castShadow = shadows;
      mesh.receiveShadow = shadows;
    }
    // Rebuilding the shadow map explicitly: three.js keeps the light's map
    // around when castShadow toggles, so without this the toggle can seem to do
    // nothing until the next scene rebuild.
    if (shadows) {
      lights.key.shadow.map?.dispose();
      lights.key.shadow.map = null;
    }

    if (lightingMode.current !== lighting) {
      lightingMode.current = lighting;
      for (const { mesh, part } of partMeshes.current) {
        mesh.material = buildPartMaterial(part, {
          lighting,
          textured,
          textures,
          wireframe,
          disabledTextures,
        });
      }
    } else {
      for (const { mesh, part } of partMeshes.current) {
        applyPartState(
          mesh.material as THREE.MeshStandardMaterial | THREE.MeshBasicMaterial,
          part,
          { textured, textures, wireframe, disabledTextures },
        );
      }
    }
  }, [
    bundle,
    wireframe,
    textured,
    textures,
    disabledTextures,
    lighting,
    light,
    shadows,
  ]);

  return <div ref={host} className="h-full w-full overflow-hidden bg-surface-0" />;
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
  options: PartMaterialOptions,
): THREE.Material {
  const { lighting, textured, textures, wireframe, disabledTextures } = options;
  const resolved =
    textured && textures
      ? resolvePartTextures(part, activeTextures(textures, disabledTextures))
      : null;

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

/** Update an existing material in place, without rebuilding the mesh. */
function applyPartState(
  material: THREE.MeshStandardMaterial | THREE.MeshBasicMaterial,
  part: MeshPart,
  options: Omit<PartMaterialOptions, "lighting">,
): void {
  const { textured, textures, wireframe, disabledTextures } = options;
  const resolved =
    textured && textures
      ? resolvePartTextures(part, activeTextures(textures, disabledTextures))
      : null;

  material.wireframe = wireframe;
  if (resolved?.diffuse) {
    material.map = resolved.diffuse;
    material.color.set(0xffffff);
  } else {
    material.map = null;
    material.color.set(0x9aa6b4);
  }
  if (material instanceof THREE.MeshStandardMaterial) {
    material.normalMap = resolved?.normal ?? null;
  }
  material.needsUpdate = true;
}

type TexturePanelProps = {
  textures: readonly ModelTexture[];
  disabled: ReadonlySet<number>;
  selected: number | null;
  onSelect: (hash: number) => void;
  onToggle: (hash: number, on: boolean) => void;
  onOpenYtd: ((path: string) => void) | undefined;
  onClose: () => void;
};

/** The texture dictionary the model's parts reference, editable per texture. */
function TexturePanel({
  textures,
  disabled,
  selected,
  onSelect,
  onToggle,
  onOpenYtd,
  onClose,
}: TexturePanelProps): React.JSX.Element {
  const shown = textures.find((t) => t.hash === selected) ?? null;

  return (
    <aside className="flex w-64 shrink-0 flex-col border-l border-edge bg-surface-0">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-edge px-3 text-xs text-ink-dim">
        <span>
          Textures · {textures.length}
          {disabled.size > 0 && `, ${disabled.size} off`}
        </span>
        <button
          type="button"
          onClick={onClose}
          className="rounded p-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
          aria-label="Close the texture panel"
        >
          <X size={13} />
        </button>
      </div>

      {shown && (
        <div className="shrink-0 border-b border-edge p-3">
          <img
            src={`data:image/png;base64,${shown.png}`}
            alt={shown.name}
            className="max-h-40 w-full object-contain [image-rendering:pixelated]"
          />
          <p className="mt-1 truncate text-xs text-ink" title={shown.name}>
            {shown.name}
          </p>
          <p className="text-xs text-ink-dim">
            {shown.width}x{shown.height}
          </p>
        </div>
      )}

      <ul className="min-h-0 flex-1 overflow-y-auto">
        {textures.map((texture) => {
          const off = disabled.has(texture.hash);
          const isSelected = texture.hash === selected;
          const source = texture.source;
          return (
            <li
              key={texture.hash}
              className={`flex items-center gap-2 px-2 py-1 text-xs ${
                off ? "opacity-45" : ""
              } ${isSelected ? "bg-surface-2" : ""}`}
            >
              <input
                type="checkbox"
                checked={!off}
                onChange={(e) => onToggle(texture.hash, e.target.checked)}
                title={off ? "Show this texture" : "Hide this texture"}
              />
              <button
                type="button"
                onClick={() => onSelect(texture.hash)}
                className="min-w-0 flex-1 truncate text-left text-ink transition-colors hover:text-accent"
                title={`${texture.name} (${texture.width}x${texture.height})`}
              >
                {texture.name}
              </button>
              {source && onOpenYtd && (
                <button
                  type="button"
                  onClick={() => onOpenYtd(source)}
                  title={`Open ${baseName(source)}`}
                  className="shrink-0 rounded p-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
                >
                  <ExternalLink size={12} strokeWidth={1.75} />
                </button>
              )}
            </li>
          );
        })}
      </ul>
    </aside>
  );
}

import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { Box, Grid3x3, Layers, Loader2, Palette } from "lucide-react";
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
    normal: normal ? textureFromPng(normal.png) : null,
  };
}

const textureCache = new Map<string, THREE.Texture>();

/** Decode one resolved texture once and keep it for the session. */
function textureFromPng(png: string): THREE.Texture {
  let cached = textureCache.get(png);
  if (cached) return cached;
  const bytes = decodeBase64(png);
  const blob = new Blob([bytes as BlobPart], { type: "image/png" });
  const url = URL.createObjectURL(blob);
  const image = new Image();
  const texture = new THREE.Texture();
  textureCache.set(png, texture);
  image.addEventListener("load", () => {
    texture.image = image;
    texture.needsUpdate = true;
    URL.revokeObjectURL(url);
  });
  image.src = url;
  return texture;
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
}: CanvasProps): React.JSX.Element {
  const host = useRef<HTMLDivElement>(null);

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

    scene.add(new THREE.HemisphereLight(0xdfe8f5, 0x33383f, 1.1));
    const key = new THREE.DirectionalLight(0xffffff, 2.4);
    key.position.set(4, 8, 6);
    key.castShadow = true;
    scene.add(key);
    // A warm low fill so the shadowed side of a prop is not pitch black.
    const fill = new THREE.DirectionalLight(0xfff2dd, 0.5);
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

      const partMaterial = new THREE.MeshStandardMaterial({
        color: 0x9aa6b4,
        roughness: 0.8,
        metalness: 0.05,
        side: THREE.DoubleSide,
      });
      partMaterial.wireframe = wireframe;

      if (textured && textures) {
        const resolved = resolvePartTextures(part, textures);
        if (resolved.diffuse) {
          partMaterial.map = resolved.diffuse;
          partMaterial.color.set(0xffffff);
          partMaterial.needsUpdate = true;
        }
        if (resolved.normal) {
          partMaterial.normalMap = resolved.normal;
          partMaterial.needsUpdate = true;
        }
      }

      const mesh = new THREE.Mesh(geometry, partMaterial);
      mesh.castShadow = true;
      mesh.receiveShadow = true;
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
  }, [bundle, wireframe, textured, textures]);

  return <div ref={host} className="min-h-0 flex-1 overflow-hidden bg-surface-0" />;
}

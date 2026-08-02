import { useEffect, useMemo, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { Box, Grid3x3, Loader2 } from "lucide-react";
import {
  decodeFloats,
  decodeUints,
  readModel,
  type MeshBundle,
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
 * three.js is the documented phase 2 choice for Explorer previews;
 * `docs/architecture.md` leaves the wgpu-versus-three decision to the phase 3
 * world renderer, which is a different problem with different constraints.
 */
export function ModelViewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<ModelPreview | null>(null);
  const [selected, setSelected] = useState(0);
  const [wireframe, setWireframe] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setSelected(0);
    readModel()
      .then((model) => {
        if (cancelled) return;
        setPreview(model);
        // Open on something that draws. A dictionary's first entry is often a
        // placeholder with no geometry, and landing on it looks like a failure.
        setSelected(model.drawables.find((d) => d.bundle !== null)?.index ?? 0);
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
  const bundle = drawable?.bundle ?? null;

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
                {d.bundle === null ? " (empty)" : ""}
              </option>
            ))}
          </select>
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

        {bundle && (
          <>
            <span className="flex items-center gap-1.5">
              <Box size={13} strokeWidth={1.75} />
              {bundle.parts.length} {bundle.parts.length === 1 ? "part" : "parts"},{" "}
              {bundle.triangleCount.toLocaleString()} triangles,{" "}
              {bundle.vertexCount.toLocaleString()} vertices
            </span>

            <span>LOD {bundle.lod}</span>
            {bundle.availableLods.length > 1 && (
              <span>({bundle.availableLods.join(", ")} available)</span>
            )}
          </>
        )}
        {preview.emptyDrawables > 0 && (
          <span>{preview.emptyDrawables} without geometry</span>
        )}
      </div>

      {bundle ? (
        <Canvas bundle={bundle} wireframe={wireframe} />
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
};

/**
 * The WebGL surface. Split out so the scene is rebuilt when the bundle changes
 * and torn down exactly once, rather than on every toolbar re-render.
 */
function Canvas({ bundle, wireframe }: CanvasProps): React.JSX.Element {
  const host = useRef<HTMLDivElement>(null);
  const material = useMemo(
    () =>
      new THREE.MeshStandardMaterial({
        color: 0x9aa6b4,
        roughness: 0.8,
        metalness: 0.05,
        // Game meshes are authored one-sided for a specific shader; without
        // this, half of a typical prop looks missing.
        side: THREE.DoubleSide,
        flatShading: false,
      }),
    [],
  );

  useEffect(() => {
    material.wireframe = wireframe;
    material.needsUpdate = true;
  }, [material, wireframe]);

  useEffect(() => {
    const container = host.current;
    if (!container) return;

    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    container.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(50, 1, 0.01, 10_000);

    scene.add(new THREE.HemisphereLight(0xdfe8f5, 0x33383f, 2.2));
    const key = new THREE.DirectionalLight(0xffffff, 1.6);
    key.position.set(4, 8, 6);
    scene.add(key);

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
      model.add(new THREE.Mesh(geometry, material));
    }

    // Frame the model from its own geometry rather than from the declared
    // bounding box: for a skinned drawable the box is in a different space.
    const box = new THREE.Box3().setFromObject(model);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    const extent = Math.max(size.x, size.y, size.z, 0.001);

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
      renderer.dispose();
      container.removeChild(renderer.domElement);
    };
  }, [bundle, material]);

  // The material outlives the scene effect, so it is disposed on unmount only.
  useEffect(() => () => material.dispose(), [material]);

  return <div ref={host} className="min-h-0 flex-1 overflow-hidden bg-surface-0" />;
}

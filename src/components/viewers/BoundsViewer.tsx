import { useEffect, useMemo, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { Box, Loader2 } from "lucide-react";
import { decodeFloats, decodeUints, readBounds, type BoundsPreview } from "../../lib/viewer";

/**
 * Preview a `.ybn` collision bounds tree in three.js.
 *
 * Same contract as `ModelViewer`: the Rust side decodes and transforms every
 * triangle into world space (`Bounds::triangle_mesh` in `cw-core`), and only
 * the resulting buffers cross the IPC boundary. The primitive shapes — spheres,
 * capsules, boxes — are listed in the toolbar rather than drawn.
 */
export function BoundsViewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<BoundsPreview | null>(null);
  const [wireframe, setWireframe] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    readBounds()
      .then((bounds) => {
        if (cancelled) return;
        setPreview(bounds);
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

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-edge px-4 text-xs text-ink-dim">
        <span>{preview.boundsType}</span>
        {preview.children > 0 && <span>{preview.children} children</span>}
        <span className="flex items-center gap-1.5">
          <Box size={13} strokeWidth={1.75} />
          {preview.parts.length} {preview.parts.length === 1 ? "part" : "parts"},{" "}
          {preview.triangleCount.toLocaleString()} triangles,{" "}
          {preview.vertexCount.toLocaleString()} vertices
        </span>
        <button
          type="button"
          onClick={() => setWireframe((on) => !on)}
          className={`rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink ${
            wireframe ? "text-accent" : ""
          }`}
        >
          Wireframe
        </button>
      </div>

      <Canvas preview={preview} wireframe={wireframe} />
    </div>
  );
}

type CanvasProps = {
  preview: BoundsPreview;
  wireframe: boolean;
};

function Canvas({ preview, wireframe }: CanvasProps): React.JSX.Element {
  const host = useRef<HTMLDivElement>(null);
  const material = useMemo(
    () =>
      new THREE.MeshStandardMaterial({
        color: 0x9aa6b4,
        roughness: 0.8,
        metalness: 0.05,
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

    // GTA V is Z-up; three.js is Y-up.
    const model = new THREE.Group();
    model.rotation.x = -Math.PI / 2;
    scene.add(model);

    const geometries: THREE.BufferGeometry[] = [];
    for (const part of preview.parts) {
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute(
        "position",
        new THREE.BufferAttribute(decodeFloats(part.positions), 3),
      );
      geometry.setIndex(new THREE.BufferAttribute(decodeUints(part.indices), 1));
      geometry.computeVertexNormals();
      geometries.push(geometry);
      model.add(new THREE.Mesh(geometry, material));
    }

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
  }, [preview, material]);

  // The material outlives the scene effect, so it is disposed on unmount only.
  useEffect(() => () => material.dispose(), [material]);

  return <div ref={host} className="min-h-0 flex-1 overflow-hidden bg-surface-0" />;
}

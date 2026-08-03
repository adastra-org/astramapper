import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as THREE from "three";
import {
  ChevronRight,
  Crosshair,
  Globe2,
  Loader2,
  MapPin,
  X,
} from "lucide-react";
import { formatHash } from "../lib/format";
import { decodeBase64, decodeFloats, decodeUints, type ModelPreview } from "../lib/viewer";
import {
  applyEdit,
  boundsOf,
  colorForLod,
  commitHistory,
  demoWorld,
  editOf,
  gtaToWorld,
  keyFor,
  lodBandForDistance,
  nearestCandidates,
  placementScale,
  worldAsset,
  worldDlcYmaps,
  worldMap,
  worldModel,
  worldStatus,
  worldToGta,
  type ContentGroup,
  type GeometryLoad,
  type HistoryEntry,
  type Placed,
  type TransformEdit,
  type WorldMap,
  type WorldStatus,
  type YmapListingPage,
} from "../lib/world";

/** Placing a map whose LOD buckets would exceed this turns shadows off. */
const SHADOW_LIMIT = 5_000;
/** How many `.ymap` rows a group's expanded list shows. */
const MAX_YMAP_ROWS = 40;
/** Entities inside this distance of the camera get geometry streamed. */
const STREAM_RADIUS = 400;
/** Entities beyond this distance have their geometry unloaded again. */
const STREAM_UNLOAD_RADIUS = 700;
/** How many entities one streaming pass may start loading. */
const STREAM_BATCH = 4;
/** How often the streaming pass re-decides what is loaded. */
const STREAM_INTERVAL_MS = 600;

export type Selection = {
  readonly mapKey: string;
  readonly entityIndex: number;
};

/**
 * World Mode: the phase 3 renderer in three.js over placements the Rust side
 * already decoded.
 *
 * The renderer decision and its trade-offs live in `docs/architecture.md`
 * under *Rendering*; the short version is that the wgpu-native-surface spike
 * lost to the documented three.js fallback. Everything here uploads buffers
 * from `src-tauri/src/world.rs` — flattened placements, positions, quaternions,
 * per-axis scales, and model geometry for the entities being streamed — and
 * nothing parses archive bytes.
 *
 * GTA V is Z-up and three.js is Y-up, so every placed object sits inside a
 * `world` group rotated `-PI / 2` about X — the same convention the model
 * viewer uses — and the camera flies in the Y-up world above it.
 */
export function World(): React.JSX.Element {
  const [status, setStatus] = useState<WorldStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checkedGroups, setCheckedGroups] = useState<ReadonlySet<string>>(new Set());
  const [pages, setPages] = useState<Readonly<Record<string, YmapListingPage>>>({});
  const [placed, setPlaced] = useState<readonly WorldMap[]>([]);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<Selection | null>(null);
  /** Entities whose real geometry is loaded, streamed or asked for by hand. */
  const [geometry, setGeometry] = useState<readonly GeometryLoad[]>([]);
  const [streaming, setStreaming] = useState(false);
  /** In-progress transform edits, applied live by the canvas, per entity key. */
  const [edits, setEdits] = useState<ReadonlyMap<string, TransformEdit>>(new Map());
  const [undo, setUndo] = useState<readonly HistoryEntry[]>([]);
  const [redo, setRedo] = useState<readonly HistoryEntry[]>([]);

  // Asset and model reads are cached so that a street of a hundred identical
  // streetlamps resolves and loads its model once, not a hundred times. The
  // caches live in refs so the streamer never rebuilds them.
  const assetCache = useRef(new Map<string, string | null>());
  const modelCache = useRef(new Map<string, ModelPreview>());
  const pending = useRef(new Set<string>());
  /** The camera's world position, written by the canvas every frame. */
  const cameraRef = useRef<{ position: [number, number, number] | null }>({ position: null });

  useEffect(() => {
    let cancelled = false;
    worldStatus()
      .then((loaded) => {
        if (cancelled) return;
        setStatus(loaded);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const toggleGroup = useCallback(async (group: string, on: boolean) => {
    setCheckedGroups((current) => {
      const next = new Set(current);
      if (on) next.add(group);
      else next.delete(group);
      return next;
    });
    if (!on) return;
    setBusy(true);
    try {
      const page = await worldDlcYmaps(group);
      setPages((current) => ({ ...current, [group]: page }));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  const place = useCallback(async (path: string) => {
    setBusy(true);
    try {
      const map = await worldMap(path);
      setPlaced((current) =>
        current.some((m) => m.path === path) ? current : [...current, map],
      );
      setError(null);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  const remove = useCallback((path: string) => {
    setPlaced((current) => current.filter((m) => m.path !== path));
    setGeometry((current) => current.filter((g) => g.mapKey !== path));
    setSelected((current) => (current && current.mapKey === path ? null : current));
  }, []);

  /** Load one entity's real geometry, resolving its archetype to a model. */
  const loadEntity = useCallback(
    async (mapKey: string, entity: Placed) => {
      const key = keyFor(mapKey, entity.index);
      if (geometry.some((g) => keyFor(g.mapKey, g.entity.index) === key)) return;
      if (pending.current.has(key)) return;
      // A name the table could not resolve has no file to load.
      if (entity.name.startsWith("hash_")) return;

      pending.current.add(key);
      try {
        let modelPath = assetCache.current.get(entity.name);
        if (modelPath === undefined) {
          modelPath = await worldAsset(entity.name);
          assetCache.current.set(entity.name, modelPath);
        }
        if (!modelPath) return;
        let model = modelCache.current.get(modelPath);
        if (!model) {
          model = await worldModel(modelPath);
          modelCache.current.set(modelPath, model);
        }
        setGeometry((current) =>
          current.some((g) => keyFor(g.mapKey, g.entity.index) === key)
            ? current
            : [...current, { mapKey, entity, modelPath, model }],
        );
      } catch (cause) {
        setError(String(cause));
      } finally {
        pending.current.delete(key);
      }
    },
    [geometry],
  );

  const unloadEntity = useCallback((mapKey: string, entityIndex: number) => {
    setGeometry((current) =>
      current.filter((g) => !(g.mapKey === mapKey && g.entity.index === entityIndex)),
    );
  }, []);

  // Live transform edits from the gizmo. Stable callbacks so the canvas can
  // call them from its pointer handlers without re-running its setup.
  const onEdit = useCallback((key: string, edit: TransformEdit) => {
    setEdits((current) => new Map(current).set(key, edit));
  }, []);

  const onEditCommit = useCallback((key: string, before: TransformEdit, after: TransformEdit) => {
    setUndo((current) => commitHistory(current, { key, before, after }));
    setRedo([]);
  }, []);

  const undoEdit = useCallback(() => {
    if (undo.length === 0) return;
    const last = undo[undo.length - 1];
    setEdits((current) => new Map(current).set(last.key, last.before));
    setUndo(undo.slice(0, -1));
    setRedo((current) => [...current, last]);
  }, [undo]);

  const redoEdit = useCallback(() => {
    if (redo.length === 0) return;
    const last = redo[redo.length - 1];
    setEdits((current) => new Map(current).set(last.key, last.after));
    setRedo(redo.slice(0, -1));
    setUndo((current) => [...current, last]);
  }, [redo]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (!(event.ctrlKey || event.metaKey)) return;
      const key = event.key.toLowerCase();
      if (key === "z") {
        if (event.shiftKey) redoEdit();
        else undoEdit();
        event.preventDefault();
      } else if (key === "y") {
        redoEdit();
        event.preventDefault();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [undoEdit, redoEdit]);

  // When no game folder is open, the same renderer shows the demo plaza so the
  // pipeline is visible (and testable) without the game.
  const demo = useMemo(
    () => (status && !status.gameOpen ? demoWorld() : null),
    [status],
  );

  const entries = useMemo(() => {
    if (demo) {
      return [{ key: "demo", name: demo.name, placements: demo.placements }];
    }
    return placed.map((map) => ({
      key: map.path,
      name: map.name,
      placements: map.entities,
    }));
  }, [demo, placed]);

  const totalPlaced = useMemo(
    () => entries.reduce((sum, entry) => sum + entry.placements.length, 0),
    [entries],
  );

  const selectedPlacement: { entry: (typeof entries)[number]; placement: Placed } | null =
    useMemo(() => {
      if (!selected) return null;
      const entry = entries.find((e) => e.key === selected.mapKey);
      if (!entry) return null;
      const placement = findPlacement(entry.placements, selected.entityIndex);
      if (!placement) return null;
      return {
        entry,
        placement: applyEdit(placement, edits.get(keyFor(selected.mapKey, selected.entityIndex))),
      };
    }, [entries, selected, edits]);

  // The streaming pass: every interval, load the nearest unloaded entities
  // within a radius of the camera, and unload the ones that have drifted too
  // far out. The camera position comes from the canvas via `cameraRef`.
  useEffect(() => {
    if (!streaming || !status?.gameOpen) return;
    const interval = window.setInterval(async () => {
      const cam = cameraRef.current.position;
      if (!cam || entries.length === 0) return;
      const cameraGta = worldToGta(cam);
      const excluded = new Set<string>();
      for (const g of geometry) excluded.add(keyFor(g.mapKey, g.entity.index));
      for (const key of pending.current) excluded.add(key);

      const candidates = nearestCandidates(
        cameraGta,
        entries,
        excluded,
        STREAM_RADIUS,
        STREAM_BATCH,
      );
      for (const candidate of candidates) {
        const entry = entries.find((e) => e.key === candidate.mapKey);
        const entity = entry?.placements[candidate.entityIndex];
        if (entry && entity) void loadEntity(candidate.mapKey, entity);
      }

      setGeometry((current) => {
        const next = current.filter((g) => {
          const dx = g.entity.position[0] - cameraGta[0];
          const dy = g.entity.position[1] - cameraGta[1];
          const dz = g.entity.position[2] - cameraGta[2];
          return Math.hypot(dx, dy, dz) <= STREAM_UNLOAD_RADIUS;
        });
        return next.length === current.length ? current : next;
      });
    }, STREAM_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, [streaming, status?.gameOpen, entries, geometry, loadEntity]);

  const selectionLoaded =
    selected !== null &&
    geometry.some(
      (g) => g.mapKey === selected.mapKey && g.entity.index === selected.entityIndex,
    );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex h-11 shrink-0 items-center gap-3 border-b border-edge px-4">
        <span className="flex items-center gap-1.5 text-xs font-semibold text-ink">
          <Globe2 size={14} strokeWidth={1.75} />
          World
        </span>
        {status?.gameOpen ? (
          <span className="truncate text-xs text-ink-dim">
            {status.folder} · {status.dlcGroups.length} content groups
          </span>
        ) : (
          <span className="text-xs text-ink-dim">
            no game folder open — showing the demo plaza
          </span>
        )}
        {busy && <Loader2 size={13} className="animate-spin text-ink-dim" />}
        {error && <span className="ml-auto truncate text-xs text-red-400">{error}</span>}
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="flex w-80 shrink-0 flex-col overflow-y-auto border-r border-edge bg-surface-1">
          {!status ? (
            <div className="flex items-center gap-2 p-4 text-xs text-ink-dim">
              <Loader2 size={13} className="animate-spin" /> reading world status…
            </div>
          ) : (
            <>
              {status.gameOpen && (
                <>
                  <Section title="Content">
                    <p className="px-4 pb-2 text-xs text-ink-dim">
                      Tick a group to list its maps; click a map to place it.
                    </p>
                    {status.dlcGroups.map((group) => (
                      <GroupRow
                        key={group.id}
                        group={group}
                        checked={checkedGroups.has(group.id)}
                        page={pages[group.id]}
                        onToggle={toggleGroup}
                        onPlace={place}
                      />
                    ))}
                    {status.openMapPath && (
                      <button
                        type="button"
                        onClick={() => void place(status.openMapPath!)}
                        disabled={busy}
                        className="mx-4 mt-2 flex items-center gap-1.5 rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-50"
                      >
                        <MapPin size={13} strokeWidth={1.75} />
                        Place the map open in Explorer
                      </button>
                    )}
                  </Section>

                  <Section title="Streaming">
                    <label className="flex cursor-pointer items-center gap-2 px-4 py-1 text-xs hover:bg-surface-2/60">
                      <input
                        type="checkbox"
                        checked={streaming}
                        onChange={(event) => setStreaming(event.target.checked)}
                        className="accent-[#e0a458]"
                      />
                      <span className="text-ink">Stream HD geometry</span>
                      <span className="ml-auto text-ink-dim">{geometry.length} loaded</span>
                    </label>
                    <p className="px-4 pb-1 text-[11px] text-ink-dim">
                      Loads the nearest placed entities' models as you fly, swapping
                      LOD by distance.
                    </p>
                  </Section>
                </>
              )}

              <Section title={`Placed (${totalPlaced.toLocaleString()})`}>
                {entries.map((entry) => (
                  <div
                    key={entry.key}
                    className="group flex items-center gap-2 px-4 py-1 text-xs hover:bg-surface-2/60"
                  >
                    <button
                      type="button"
                      onClick={() => setSelected(null)}
                      className={`min-w-0 flex-1 truncate text-left ${
                        selected?.mapKey === entry.key ? "text-accent" : "text-ink"
                      }`}
                    >
                      {entry.name || entry.key}
                    </button>
                    <span className="shrink-0 text-ink-dim">
                      {entry.placements.length.toLocaleString()}
                    </span>
                    {!demo && (
                      <button
                        type="button"
                        onClick={() => remove(entry.key)}
                        aria-label={`remove ${entry.name}`}
                        className="shrink-0 rounded p-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-red-400"
                      >
                        <X size={12} strokeWidth={1.75} />
                      </button>
                    )}
                  </div>
                ))}
                {entries.length === 0 && (
                  <p className="px-4 py-2 text-xs text-ink-dim">
                    Nothing placed yet. Pick a map from a content group above.
                  </p>
                )}
              </Section>

              <Section title="History">
                <div className="flex items-center gap-2 px-4 py-1">
                  <button
                    type="button"
                    onClick={undoEdit}
                    disabled={undo.length === 0}
                    className="rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
                  >
                    Undo
                  </button>
                  <button
                    type="button"
                    onClick={redoEdit}
                    disabled={redo.length === 0}
                    className="rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
                  >
                    Redo
                  </button>
                  <span className="ml-auto text-[11px] text-ink-dim">
                    Ctrl+Z / Ctrl+Y
                  </span>
                </div>
                <p className="px-4 pb-1 text-[11px] text-ink-dim">
                  Drag a gizmo axis to move the selected entity; each drag is one
                  undo step.
                </p>
              </Section>

              <Section title="Selection">
                {selectedPlacement ? (
                  <div className="space-y-1 px-4 py-2 text-xs">
                    <div className="flex items-center gap-1.5">
                      <Crosshair size={12} strokeWidth={1.75} className="text-accent" />
                      <span className="min-w-0 flex-1 truncate text-ink">
                        {selectedPlacement.placement.name || formatHash(selectedPlacement.placement.nameHash)}
                      </span>
                    </div>
                    <Detail label="map" value={selectedPlacement.entry.name} />
                    <Detail label="index" value={selectedPlacement.placement.index} />
                    <Detail label="lod" value={selectedPlacement.placement.lodLevel} />
                    <Detail
                      label="position"
                      value={selectedPlacement.placement.position.map((n) => n.toFixed(1)).join(", ")}
                    />
                    <Detail
                      label="rotation"
                      value={selectedPlacement.placement.rotation.map((n) => n.toFixed(3)).join(", ")}
                    />
                    <Detail
                      label="scale"
                      value={`${selectedPlacement.placement.scaleXy.toFixed(2)} / ${selectedPlacement.placement.scaleZ.toFixed(2)}`}
                    />
                    <Detail label="parent" value={selectedPlacement.placement.parentIndex} />
                    <Detail label="children" value={selectedPlacement.placement.children} />
                    {selectedPlacement.placement.isMlo && (
                      <span className="rounded bg-surface-2 px-1 py-px text-[10px] text-ink-dim">
                        interior instance
                      </span>
                    )}
                    {status?.gameOpen && !selectedPlacement.placement.name.startsWith("hash_") && (
                      <button
                        type="button"
                        onClick={() =>
                          selectionLoaded
                            ? unloadEntity(
                                selectedPlacement.entry.key,
                                selectedPlacement.placement.index,
                              )
                            : void loadEntity(
                                selectedPlacement.entry.key,
                                selectedPlacement.placement,
                              )
                        }
                        className="mt-1 flex items-center gap-1.5 rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70"
                      >
                        {selectionLoaded ? "Hide model" : "Show model"}
                      </button>
                    )}
                  </div>
                ) : (
                  <p className="px-4 py-2 text-xs text-ink-dim">
                    Click a placed box to inspect it. Drag to look around, WASD to
                    fly, Shift to sprint, Space/Ctrl up/down, wheel to dolly.
                  </p>
                )}
              </Section>
            </>
          )}
        </aside>

        <Canvas
          entries={entries}
          geometry={geometry}
          cameraRef={cameraRef}
          edits={edits}
          onEdit={onEdit}
          onEditCommit={onEditCommit}
          selected={selected}
          onSelect={setSelected}
          shadowed={totalPlaced <= SHADOW_LIMIT}
        />
      </div>
    </div>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}): React.JSX.Element {
  return (
    <section className="border-b border-edge py-2">
      <h2 className="px-4 pb-1 text-[11px] font-semibold uppercase tracking-wide text-ink-dim">
        {title}
      </h2>
      {children}
    </section>
  );
}

function Detail({ label, value }: { label: string; value: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex gap-2 py-px">
      <span className="w-16 shrink-0 text-ink-dim">{label}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-ink">{value}</span>
    </div>
  );
}

function GroupRow({
  group,
  checked,
  page,
  onToggle,
  onPlace,
}: {
  group: ContentGroup;
  checked: boolean;
  page: YmapListingPage | undefined;
  onToggle: (group: string, on: boolean) => void;
  onPlace: (path: string) => void;
}): React.JSX.Element {
  return (
    <div>
      <label className="flex cursor-pointer items-center gap-2 px-4 py-1 text-xs hover:bg-surface-2/60">
        <input
          type="checkbox"
          checked={checked}
          onChange={(event) => void onToggle(group.id, event.target.checked)}
          className="accent-[#e0a458]"
        />
        <span className="min-w-0 flex-1 truncate text-ink">{group.label}</span>
        <span className="shrink-0 text-ink-dim">
          {group.archives} · {group.entries.toLocaleString()}
        </span>
      </label>
      {checked && page && page.ymaps.length > 0 && (
        <div className="ml-6 border-l border-edge py-0.5 pl-2">
          {page.ymaps.slice(0, MAX_YMAP_ROWS).map((ymap) => (
            <button
              key={ymap.path}
              type="button"
              onClick={() => void onPlace(ymap.path)}
              className="flex w-full items-center gap-1.5 px-2 py-0.5 text-left text-xs text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
            >
              <ChevronRight size={10} className="shrink-0" />
              <span className="min-w-0 flex-1 truncate">{ymap.name}</span>
            </button>
          ))}
          {page.total > page.ymaps.length && (
            <p className="px-2 py-0.5 text-[10px] text-ink-dim">
              and {page.total - page.ymaps.length} more maps
            </p>
          )}
        </div>
      )}
    </div>
  );
}

type CanvasEntry = {
  readonly key: string;
  readonly name: string;
  readonly placements: readonly Placed[];
};

type CanvasProps = {
  entries: readonly CanvasEntry[];
  geometry: readonly GeometryLoad[];
  cameraRef: { readonly current: { position: [number, number, number] | null } };
  edits: ReadonlyMap<string, TransformEdit>;
  onEdit: (key: string, edit: TransformEdit) => void;
  onEditCommit: (key: string, before: TransformEdit, after: TransformEdit) => void;
  selected: Selection | null;
  onSelect: (selection: Selection | null) => void;
  shadowed: boolean;
};

/** A hit produced by the raycast, tied back to the placement it landed on. */
type Hit = {
  readonly mapKey: string;
  readonly entityIndex: number;
  readonly distance: number;
};

/** One box instanced mesh, with the placement index each instance carries. */
type BoxMesh = {
  readonly mesh: THREE.InstancedMesh;
  readonly instances: readonly number[];
};

/** The LOD-swap state for one loaded entity. */
type LoadedGeometry = {
  readonly holder: THREE.Group;
  readonly lodGroups: readonly THREE.Mesh[][];
  readonly lodDist: number;
  readonly positionGta: readonly [number, number, number];
};

/** One move-gizmo axis, in the game's Z-up space. */
type GizmoAxis = {
  /** The axis in GTA space, the direction an edit moves along. */
  readonly dirGta: readonly [number, number, number];
  readonly color: number;
};

/**
 * The three.js surface. Set up once per `entries` change: instanced boxes for
 * every placement bucketed by colour, a grid, and a Y-up fly camera. The loaded
 * geometry is managed by a separate effect so streaming a model in or out does
 * not rebuild the scene — and so the camera never resets mid-flight.
 */
function Canvas({
  entries,
  geometry,
  cameraRef,
  edits,
  onEdit,
  onEditCommit,
  selected,
  onSelect,
  shadowed,
}: CanvasProps): React.JSX.Element {
  const host = useRef<HTMLDivElement>(null);
  const entriesRef = useRef(entries);
  entriesRef.current = entries;
  const editsRef = useRef(edits);
  editsRef.current = edits;
  const selectedRef = useRef(selected);
  selectedRef.current = selected;

  const worldGroup = useRef<THREE.Group | null>(null);
  const geometryGroup = useRef<THREE.Group | null>(null);
  const highlight = useRef<THREE.LineSegments | null>(null);
  const boxByMap = useRef(new Map<string, BoxMesh>());
  const geometryMeshes = useRef<{ mesh: THREE.Mesh; mapKey: string; entityIndex: number }[]>([]);
  const loadedState = useRef(new Map<string, LoadedGeometry>());
  const lodUpdate = useRef<((cameraWorld: THREE.Vector3) => void) | null>(null);
  /** Selected entity key → its map and entity index, for the gizmo and edits. */
  const keyLookup = useRef(new Map<string, { mapKey: string; entityIndex: number }>());
  /** The gizmo's world-space origin, kept for screen-space axis picking. */
  const gizmoOriginWorld = useRef<THREE.Vector3 | null>(null);

  useEffect(() => {
    const container = host.current;
    if (!container) return;

    const renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.shadowMap.enabled = shadowed;
    renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    container.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x10141c);

    const camera = new THREE.PerspectiveCamera(60, 1, 0.1, 200_000);
    camera.position.set(80, 120, 160);

    scene.add(new THREE.HemisphereLight(0xdfe8f5, 0x33383f, 0.9));
    const sun = new THREE.DirectionalLight(0xffffff, 1.6);
    sun.position.set(200, 400, 300);
    if (shadowed) {
      sun.castShadow = true;
      sun.shadow.mapSize.set(2048, 2048);
    }
    scene.add(sun);

    // The whole world lives in a Z-up group, rotated into three.js's Y-up world
    // exactly as the model viewer rotates a single drawable.
    const world = new THREE.Group();
    world.rotation.x = -Math.PI / 2;
    scene.add(world);
    worldGroup.current = world;

    // Streamed model geometry lives in its own child group so the streaming
    // effect can swap its contents without touching boxes or the camera.
    const geometryGroupObject = new THREE.Group();
    world.add(geometryGroupObject);
    geometryGroup.current = geometryGroupObject;

    const grid = new THREE.GridHelper(400, 40, 0x3a4250, 0x2a303a);
    scene.add(grid);

    // The selection outline, shown and placed by the selection effect.
    const outline = new THREE.LineSegments(
      new THREE.EdgesGeometry(new THREE.BoxGeometry(1, 1, 1)),
      new THREE.LineBasicMaterial({ color: 0xffe066 }),
    );
    outline.visible = false;
    outline.renderOrder = 10;
    world.add(outline);
    highlight.current = outline;

    const unitGeometry = new THREE.BoxGeometry(1, 1, 1);
    const boxMeshes: { mesh: THREE.InstancedMesh; mapKey: string; indexes: number[] }[] = [];
    const boxes = new Map<string, BoxMesh>();

    // One InstancedMesh per map-and-colour bucket. Three.js gives instanced
    // meshes a single material, so buckets are how the LOD colours show; the
    // parallel `indexes` list maps each instance back to its entity.
    for (const entry of entries) {
      const byColour = new Map<number, Placed[]>();
      for (const placement of entry.placements) {
        const colour = colorForLod(placement.lodLevel, placement.isMlo);
        const bucket = byColour.get(colour) ?? [];
        bucket.push(placement);
        byColour.set(colour, bucket);
      }
      for (const [colour, placements] of byColour) {
        const material = new THREE.MeshStandardMaterial({
          color: colour,
          roughness: 0.9,
          metalness: 0,
          side: THREE.DoubleSide,
        });
        const mesh = new THREE.InstancedMesh(unitGeometry, material, placements.length);
        const matrix = new THREE.Matrix4();
        const position = new THREE.Vector3();
        const quaternion = new THREE.Quaternion();
        const scale = new THREE.Vector3();
        const indexes: number[] = [];
        for (let i = 0; i < placements.length; i += 1) {
          const p = placements[i];
          position.set(p.position[0], p.position[1], p.position[2]);
          quaternion.set(p.rotation[0], p.rotation[1], p.rotation[2], p.rotation[3]);
          const [sx, sy, sz] = placementScale(p.scaleXy, p.scaleZ);
          scale.set(sx, sy, sz);
          matrix.compose(position, quaternion, scale);
          mesh.setMatrixAt(i, matrix);
          indexes.push(p.index);
        }
        mesh.instanceMatrix.needsUpdate = true;
        mesh.castShadow = shadowed;
        mesh.receiveShadow = shadowed;
        mesh.computeBoundingSphere();
        world.add(mesh);
        boxMeshes.push({ mesh, mapKey: entry.key, indexes });
      }
      // One box entry per map: the first instanced mesh for that map.
      const first = boxMeshes.find((b) => b.mapKey === entry.key);
      if (first) {
        boxes.set(entry.key, { mesh: first.mesh, instances: first.indexes });
      }
    }
    boxByMap.current = boxes;

    // Every placement's identity, so edits can be applied by key each frame.
    const lookup = new Map<string, { mapKey: string; entityIndex: number }>();
    for (const entry of entries) {
      for (const placement of entry.placements) {
        lookup.set(keyFor(entry.key, placement.index), {
          mapKey: entry.key,
          entityIndex: placement.index,
        });
      }
    }
    keyLookup.current = lookup;

    // The move gizmo: three coloured axis lines in the game's Z-up space,
    // hidden until an entity is selected. Picking is done in screen space, so
    // the lines carry no hit geometry.
    const GIZMO_AXES: GizmoAxis[] = [
      { dirGta: [1, 0, 0], color: 0xff4d4d },
      { dirGta: [0, 1, 0], color: 0x4dff4d },
      { dirGta: [0, 0, 1], color: 0x4d8dff },
    ];
    const gizmo = new THREE.Group();
    gizmo.visible = false;
    world.add(gizmo);
    const GIZMO_LENGTH = 30;
    for (const axis of GIZMO_AXES) {
      const points = [
        new THREE.Vector3(),
        new THREE.Vector3(
          axis.dirGta[0] * GIZMO_LENGTH,
          axis.dirGta[1] * GIZMO_LENGTH,
          axis.dirGta[2] * GIZMO_LENGTH,
        ),
      ];
      const lineGeometry = new THREE.BufferGeometry().setFromPoints(points);
      const line = new THREE.Line(
        lineGeometry,
        new THREE.LineBasicMaterial({ color: axis.color, linewidth: 2 }),
      );
      gizmo.add(line);
    }
    gizmoOriginWorld.current = null;

    // Frame the camera over everything that was placed.
    const { center, radius } = boundsOf(entries.flatMap((entry) => entry.placements));
    const [fx, fy, fz] = gtaToWorld(center);
    const framed = new THREE.Vector3(fx, fy, fz);
    const offset = new THREE.Vector3(1, 0.7, 1).normalize().multiplyScalar(radius * 1.6);
    camera.position.copy(framed).add(offset);
    camera.lookAt(framed);

    const raycaster = new THREE.Raycaster();
    const pointer = new THREE.Vector2();

    // The fly camera. Yaw spins around world Y, pitch tilts the view; WASD
    // moves in the ground plane, Space/Ctrl climb, Shift sprints.
    const state = {
      yaw: Math.atan2(camera.position.x - framed.x, camera.position.z - framed.z),
      pitch: -Math.asin((camera.position.y - framed.y) / Math.max(camera.position.distanceTo(framed), 1e-6)),
      position: camera.position.clone(),
      pressed: new Set<string>(),
      dragging: false,
      moved: 0,
      lastX: 0,
      lastY: 0,
      // A gizmo axis drag, when one is live.
      gizmoAxis: -1,
      gizmoKey: "",
      gizmoStart: null as TransformEdit | null,
      gizmoCurrent: null as TransformEdit | null,
      gizmoPlane: null as THREE.Plane | null,
      gizmoLastPoint: null as THREE.Vector3 | null,
      gizmoMoved: false,
    };

    const keys = new Map<string, string>([
      ["KeyW", "forward"],
      ["KeyS", "back"],
      ["KeyA", "left"],
      ["KeyD", "right"],
      ["Space", "up"],
      ["ControlLeft", "down"],
      ["ShiftLeft", "sprint"],
    ]);

    const onKeyDown = (event: KeyboardEvent): void => {
      const action = keys.get(event.code);
      if (!action) return;
      event.preventDefault();
      state.pressed.add(action);
    };
    const onKeyUp = (event: KeyboardEvent): void => {
      const action = keys.get(event.code);
      if (action) state.pressed.delete(action);
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);

    /** The current effective edit for an entity, or the base transform. */
    const effectiveEdit = (key: string): TransformEdit | null => {
      const target = keyLookup.current.get(key);
      if (!target) return null;
      const entry = entriesRef.current.find((e) => e.key === target.mapKey);
      const placement = entry && findPlacement(entry.placements, target.entityIndex);
      if (!placement) return null;
      return editsRef.current.get(key) ?? editOf(placement);
    };

    const projectToScreen = (point: THREE.Vector3): [number, number] => {
      const ndc = point.clone().project(camera);
      return [
        (ndc.x * 0.5 + 0.5) * renderer.domElement.clientWidth,
        (-ndc.y * 0.5 + 0.5) * renderer.domElement.clientHeight,
      ];
    };

    const distanceToSegment = (
      px: number,
      py: number,
      ax: number,
      ay: number,
      bx: number,
      by: number,
    ): number => {
      const dx = bx - ax;
      const dy = by - ay;
      const lengthSq = dx * dx + dy * dy;
      const t =
        lengthSq === 0
          ? 0
          : THREE.MathUtils.clamp(((px - ax) * dx + (py - ay) * dy) / lengthSq, 0, 1);
      const cx = ax + t * dx;
      const cy = ay + t * dy;
      return Math.hypot(px - cx, py - cy);
    };

    /** Which gizmo axis is under the pointer, by screen distance. */
    const pickGizmoAxis = (clientX: number, clientY: number): number => {
      const origin = gizmoOriginWorld.current;
      if (!origin) return -1;
      let best = -1;
      let bestDistance = 14;
      for (let i = 0; i < GIZMO_AXES.length; i += 1) {
        const dir = GIZMO_AXES[i].dirGta;
        const [wx, wy, wz] = gtaToWorld(dir);
        const end = new THREE.Vector3(origin.x + wx * GIZMO_LENGTH, origin.y + wy * GIZMO_LENGTH, origin.z + wz * GIZMO_LENGTH);
        const [sx0, sy0] = projectToScreen(origin);
        const [sx1, sy1] = projectToScreen(end);
        const distance = distanceToSegment(clientX, clientY, sx0, sy0, sx1, sy1);
        if (distance < bestDistance) {
          bestDistance = distance;
          best = i;
        }
      }
      return best;
    };

    /** The ray through the pointer, for gizmo plane math. */
    const rayThrough = (clientX: number, clientY: number): THREE.Ray => {
      const rect = renderer.domElement.getBoundingClientRect();
      pointer.x = ((clientX - rect.left) / rect.width) * 2 - 1;
      pointer.y = -((clientY - rect.top) / rect.height) * 2 + 1;
      raycaster.setFromCamera(pointer, camera);
      return raycaster.ray;
    };

    const onPointerDown = (event: PointerEvent): void => {
      if (event.button !== 0) return;
      state.moved = 0;
      state.lastX = event.clientX;
      state.lastY = event.clientY;

      // A selected entity owns the left button: grab its gizmo axis, or fall
      // back to rotating the camera.
      const selection = selectedRef.current;
      if (selection) {
        const axis = pickGizmoAxis(event.clientX, event.clientY);
        if (axis >= 0) {
          const key = keyFor(selection.mapKey, selection.entityIndex);
          const start = effectiveEdit(key);
          if (start) {
            state.gizmoAxis = axis;
            state.gizmoKey = key;
            state.gizmoStart = start;
            state.gizmoCurrent = start;
            const origin = gizmoOriginWorld.current!;
            const normal = camera.getWorldDirection(new THREE.Vector3());
            state.gizmoPlane = new THREE.Plane().setFromNormalAndCoplanarPoint(normal, origin);
            state.gizmoLastPoint = new THREE.Vector3();
            rayThrough(event.clientX, event.clientY).intersectPlane(state.gizmoPlane, state.gizmoLastPoint);
            state.gizmoMoved = false;
            return;
          }
        }
      }
      state.dragging = true;
    };

    const onPointerMove = (event: PointerEvent): void => {
      if (state.gizmoAxis >= 0) {
        const point = new THREE.Vector3();
        if (!state.gizmoPlane || !rayThrough(event.clientX, event.clientY).intersectPlane(state.gizmoPlane, point)) {
          return;
        }
        state.moved += Math.abs(event.clientX - state.lastX) + Math.abs(event.clientY - state.lastY);
        state.lastX = event.clientX;
        state.lastY = event.clientY;
        const axis = GIZMO_AXES[state.gizmoAxis];
        const [wx, wy, wz] = gtaToWorld(axis.dirGta);
        const direction = new THREE.Vector3(wx, wy, wz);
        const delta = point.sub(state.gizmoLastPoint!).dot(direction);
        state.gizmoLastPoint = point;
        state.gizmoMoved = state.gizmoMoved || Math.abs(delta) > 1e-6;
        const start = state.gizmoStart!;
        const position: [number, number, number] = [
          start.position[0] + axis.dirGta[0] * delta,
          start.position[1] + axis.dirGta[1] * delta,
          start.position[2] + axis.dirGta[2] * delta,
        ];
        const edit: TransformEdit = { ...start, position };
        state.gizmoCurrent = edit;
        onEdit(state.gizmoKey, edit);
        return;
      }
      if (!state.dragging) return;
      const dx = event.clientX - state.lastX;
      const dy = event.clientY - state.lastY;
      state.lastX = event.clientX;
      state.lastY = event.clientY;
      state.moved += Math.abs(dx) + Math.abs(dy);
      state.yaw -= dx * 0.005;
      state.pitch = THREE.MathUtils.clamp(state.pitch + dy * 0.005, -1.55, 1.55);
    };

    const onPointerUp = (): void => {
      if (state.gizmoAxis >= 0) {
        if (state.gizmoMoved && state.gizmoStart && state.gizmoCurrent) {
          onEditCommit(state.gizmoKey, state.gizmoStart, state.gizmoCurrent);
        }
        state.dragging = false;
        state.gizmoAxis = -1;
        state.gizmoKey = "";
        state.gizmoStart = null;
        state.gizmoCurrent = null;
        state.gizmoPlane = null;
        state.gizmoLastPoint = null;
        state.gizmoMoved = false;
        return;
      }
      state.dragging = false;
    };
    const onWheel = (event: WheelEvent): void => {
      event.preventDefault();
      const look = camera.getWorldDirection(new THREE.Vector3());
      state.position.addScaledVector(look, event.deltaY * 0.05);
    };
    renderer.domElement.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    renderer.domElement.addEventListener("wheel", onWheel, { passive: false });

    const frameAt = (gta: readonly [number, number, number], r: number): void => {
      const [wx, wy, wz] = gtaToWorld(gta);
      const target = new THREE.Vector3(wx, wy, wz);
      const dir = new THREE.Vector3(1, 0.7, 1).normalize().multiplyScalar(Math.max(r * 2.2, 10));
      state.position.copy(target).add(dir);
      // Same convention as the initial framing: the camera looks from `dir`
      // towards the target, so its forward is `-dir`.
      state.yaw = Math.atan2(dir.x, dir.z);
      state.pitch = -Math.asin(dir.y / dir.length());
    };

    const pick = (clientX: number, clientY: number): Hit | null => {
      const rect = renderer.domElement.getBoundingClientRect();
      pointer.x = ((clientX - rect.left) / rect.width) * 2 - 1;
      pointer.y = -((clientY - rect.top) / rect.height) * 2 + 1;
      raycaster.setFromCamera(pointer, camera);
      let best: Hit | null = null;
      for (const { mesh, mapKey, indexes } of boxMeshes) {
        const hits = raycaster.intersectObject(mesh, false);
        const first = hits[0];
        if (!first) continue;
        if (first.instanceId === undefined) continue;
        if (!best || first.distance < best.distance) {
          best = { mapKey, entityIndex: indexes[first.instanceId], distance: first.distance };
        }
      }
      for (const { mesh, mapKey, entityIndex } of geometryMeshes.current) {
        const hits = raycaster.intersectObject(mesh, false);
        const first = hits[0];
        if (!first) continue;
        if (!best || first.distance < best.distance) {
          best = { mapKey, entityIndex, distance: first.distance };
        }
      }
      return best;
    };

    const onClick = (event: MouseEvent): void => {
      // A drag ends in a click too; only a press that did not move the camera
      // counts as a selection.
      if (state.moved > 4) return;
      const hit = pick(event.clientX, event.clientY);
      onSelect(hit ? { mapKey: hit.mapKey, entityIndex: hit.entityIndex } : null);
    };
    const onDoubleClick = (event: MouseEvent): void => {
      const hit = pick(event.clientX, event.clientY);
      if (!hit) return;
      const entry = entriesRef.current.find((e) => e.key === hit.mapKey);
      const placement = findPlacement(entry?.placements, hit.entityIndex);
      if (!placement) return;
      frameAt(placement.position, 40);
    };
    renderer.domElement.addEventListener("click", onClick);
    renderer.domElement.addEventListener("dblclick", onDoubleClick);

    const clock = new THREE.Clock();
    let frame = 0;
    const tick = () => {
      frame = requestAnimationFrame(tick);
      const dt = Math.min(clock.getDelta(), 0.05);
      const baseSpeed = 60;
      const speed = baseSpeed * (state.pressed.has("sprint") ? 4 : 1) * dt;
      const forward = new THREE.Vector3(-Math.sin(state.yaw), 0, -Math.cos(state.yaw));
      const right = new THREE.Vector3(Math.cos(state.yaw), 0, -Math.sin(state.yaw));
      if (state.pressed.has("forward")) state.position.addScaledVector(forward, speed);
      if (state.pressed.has("back")) state.position.addScaledVector(forward, -speed);
      if (state.pressed.has("left")) state.position.addScaledVector(right, -speed);
      if (state.pressed.has("right")) state.position.addScaledVector(right, speed);
      if (state.pressed.has("up")) state.position.y += speed;
      if (state.pressed.has("down")) state.position.y -= speed;

      camera.position.copy(state.position);
      const look = new THREE.Vector3(
        -Math.sin(state.yaw) * Math.cos(state.pitch),
        Math.sin(state.pitch),
        -Math.cos(state.yaw) * Math.cos(state.pitch),
      );
      camera.lookAt(state.position.clone().add(look));
      cameraRef.current.position = [camera.position.x, camera.position.y, camera.position.z];

      lodUpdate.current?.(camera.position);
      applyEdits();
      updateGizmo();
      renderer.render(scene, camera);
    };

    /** Lay every live edit onto its box instance or geometry holder. */
    const applyEdits = (): void => {
      for (const [key, edit] of editsRef.current) {
        const target = keyLookup.current.get(key);
        if (!target) continue;
        const loaded = loadedState.current.get(key);
        if (loaded) {
          loaded.holder.position.set(edit.position[0], edit.position[1], edit.position[2]);
          loaded.holder.quaternion.set(
            edit.rotation[0],
            edit.rotation[1],
            edit.rotation[2],
            edit.rotation[3],
          );
          const [sx, sy, sz] = placementScale(edit.scaleXy, edit.scaleZ);
          loaded.holder.scale.set(sx, sy, sz);
        } else {
          const box = boxByMap.current.get(target.mapKey);
          if (box) setInstanceEdit(box.mesh, box.instances, target.entityIndex, edit);
        }
      }
    };

    /** Park the move gizmo on the selected entity's edited position. */
    const updateGizmo = (): void => {
      const selection = selectedRef.current;
      if (!selection) {
        gizmo.visible = false;
        gizmoOriginWorld.current = null;
        return;
      }
      const edit = effectiveEdit(keyFor(selection.mapKey, selection.entityIndex));
      if (!edit) {
        gizmo.visible = false;
        gizmoOriginWorld.current = null;
        return;
      }
      gizmo.position.set(edit.position[0], edit.position[1], edit.position[2]);
      gizmo.visible = true;
      const [wx, wy, wz] = gtaToWorld(edit.position);
      gizmoOriginWorld.current = new THREE.Vector3(wx, wy, wz);
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
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      renderer.domElement.removeEventListener("pointerdown", onPointerDown);
      renderer.domElement.removeEventListener("wheel", onWheel);
      renderer.domElement.removeEventListener("click", onClick);
      renderer.domElement.removeEventListener("dblclick", onDoubleClick);
      disposeObject3D(geometryGroupObject);
      disposeObject3D(gizmo);
      worldGroup.current = null;
      geometryGroup.current = null;
      highlight.current = null;
      boxByMap.current = new Map();
      geometryMeshes.current = [];
      loadedState.current = new Map();
      lodUpdate.current = null;
      unitGeometry.dispose();
      for (const { mesh } of boxMeshes) {
        (mesh.material as THREE.Material).dispose();
        mesh.dispose();
      }
      grid.dispose();
      sun.shadow?.map?.dispose();
      renderer.dispose();
      container.removeChild(renderer.domElement);
    };
    // `cameraRef` is a stable ref owned by the parent; writing its `position`
    // here is how the streamer reads the camera, not a reactive dependency.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [entries, shadowed, onSelect]);

  // Streamed geometry: rebuild only the geometry group when the loaded set
  // changes, hiding and restoring the matching box instances so a marker does
  // not sit on top of its model.
  useEffect(() => {
    const world = worldGroup.current;
    const group = geometryGroup.current;
    if (!world || !group) return;

    disposeObject3D(group);
    geometryMeshes.current = [];
    const state = new Map<string, LoadedGeometry>();
    const cameraWorld = new THREE.Vector3();
    const matrix = new THREE.Matrix4();
    const position = new THREE.Vector3();
    const quaternion = new THREE.Quaternion();
    const scale = new THREE.Vector3();

    const geometryKeys = new Set(geometry.map((g) => keyFor(g.mapKey, g.entity.index)));

    for (const load of geometry) {
      const key = keyFor(load.mapKey, load.entity.index);
      const holder = new THREE.Group();
      position.set(load.entity.position[0], load.entity.position[1], load.entity.position[2]);
      quaternion.set(
        load.entity.rotation[0],
        load.entity.rotation[1],
        load.entity.rotation[2],
        load.entity.rotation[3],
      );
      const [sx, sy, sz] = placementScale(load.entity.scaleXy, load.entity.scaleZ);
      scale.set(sx, sy, sz);
      matrix.compose(position, quaternion, scale);
      holder.applyMatrix4(matrix);
      holder.updateMatrixWorld();

      const lodGroups = buildLodMeshes(load, shadowed);
      for (const meshes of lodGroups) {
        for (const mesh of meshes) {
          mesh.userData.mapKey = load.mapKey;
          mesh.userData.entityIndex = load.entity.index;
          holder.add(mesh);
          geometryMeshes.current.push({ mesh, mapKey: load.mapKey, entityIndex: load.entity.index });
        }
      }
      group.add(holder);
      state.set(key, {
        holder,
        lodGroups,
        lodDist: load.entity.lodDist,
        positionGta: load.entity.position,
      });

      // The entity now has real geometry; its box marker must step aside. A
      // model that produced no geometry keeps its marker rather than vanishing.
      if (lodGroups.length > 0) {
        const box = boxByMap.current.get(load.mapKey);
        if (box) setInstanceScale(box.mesh, box.instances, load.entity.index, 0);
      }
    }

    // Any box whose entity is no longer loaded comes back.
    for (const entry of entriesRef.current) {
      const box = boxByMap.current.get(entry.key);
      if (!box) continue;
      for (const placement of entry.placements) {
        const key = keyFor(entry.key, placement.index);
        if (geometryKeys.has(key)) continue;
        setInstanceTransform(
          box.mesh,
          box.instances,
          placement.index,
          placement,
        );
      }
    }

    loadedState.current = state;
    lodUpdate.current = (cameraPosition) => {
      cameraWorld.copy(cameraPosition);
      const [cx, cy, cz] = worldToGta([cameraWorld.x, cameraWorld.y, cameraWorld.z]);
      for (const { lodGroups, lodDist, positionGta } of state.values()) {
        const dist = Math.hypot(
          positionGta[0] - cx,
          positionGta[1] - cy,
          positionGta[2] - cz,
        );
        const band = lodBandForDistance(dist, lodDist, lodGroups.length);
        for (let i = 0; i < lodGroups.length; i += 1) {
          const visible = i === band;
          for (const mesh of lodGroups[i]) mesh.visible = visible;
        }
      }
    };
    // `entries` is in the deps because the setup effect tears the geometry
    // group down when the placed maps change; this effect must repopulate it.
  }, [geometry, entries, shadowed]);

  // Apply the selection to the outline without rebuilding the scene.
  useEffect(() => {
    const world = worldGroup.current;
    const outline = highlight.current;
    if (!world || !outline) return;
    if (!selected) {
      outline.visible = false;
      return;
    }
    const entry = entriesRef.current.find((e) => e.key === selected.mapKey);
    const base = findPlacement(entry?.placements, selected.entityIndex);
    if (!base) {
      outline.visible = false;
      return;
    }
    // The outline follows the edit, so it tracks the gizmo while dragging.
    const placement = applyEdit(base, edits.get(keyFor(selected.mapKey, selected.entityIndex)));
    const matrix = new THREE.Matrix4();
    const position = new THREE.Vector3(placement.position[0], placement.position[1], placement.position[2]);
    const quaternion = new THREE.Quaternion(
      placement.rotation[0],
      placement.rotation[1],
      placement.rotation[2],
      placement.rotation[3],
    );
    const [sx, sy, sz] = placementScale(placement.scaleXy, placement.scaleZ);
    matrix.compose(position, quaternion, new THREE.Vector3(sx * 1.03, sy * 1.03, sz * 1.03));
    outline.matrixAutoUpdate = false;
    outline.matrix.copy(matrix);
    outline.visible = true;
  }, [selected, edits]);

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={host} className="min-h-0 h-full bg-surface-0" />
      {totalCount(entries) === 0 && (
        <p className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-ink-dim">
          The world is empty. Place a map to fill it.
        </p>
      )}
    </div>
  );
}

function totalCount(entries: readonly CanvasEntry[]): number {
  return entries.reduce((sum, entry) => sum + entry.placements.length, 0);
}

/**
 * A placement by its file index. Raycasting returns the instance's original
 * entity index, which is the identity — not an array position, so a filtered
 * or reordered list would silently select the wrong thing.
 */
function findPlacement(
  placements: readonly Placed[] | undefined,
  index: number,
): Placed | undefined {
  return placements?.find((placement) => placement.index === index);
}

/**
 * The three.js meshes for one load's LODs, one array per bundle. The bundle
 * with geometry is the first drawable that has any; everything else in a `.ydd`
 * or fragment is not drawn here.
 */
function buildLodMeshes(load: GeometryLoad, shadowed: boolean): THREE.Mesh[][] {
  const drawable = load.model.drawables.find((d) => d.lods.length > 0);
  if (!drawable) return [];
  return drawable.lods.map((bundle) => {
    const meshes: THREE.Mesh[] = [];
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
      const material = new THREE.MeshStandardMaterial({
        color: 0xb9c0ca,
        roughness: 0.9,
        metalness: 0,
        side: THREE.DoubleSide,
      });
      const resolved = resolvePartTextures(part, load.model.textures);
      if (resolved.diffuse) {
        material.map = resolved.diffuse;
        material.color.set(0xffffff);
        material.needsUpdate = true;
      }
      if (resolved.normal) {
        material.normalMap = resolved.normal;
        material.needsUpdate = true;
      }
      const mesh = new THREE.Mesh(geometry, material);
      mesh.castShadow = shadowed;
      mesh.receiveShadow = shadowed;
      meshes.push(mesh);
    }
    return meshes;
  });
}

/** The texture map for one part, resolved against the model's decoded PNGs. */
function resolvePartTextures(
  part: { readonly diffuse: number | null; readonly normal: number | null },
  textures: readonly { readonly hash: number; readonly png: string }[],
): { diffuse: THREE.Texture | null; normal: THREE.Texture | null } {
  const byHash = new Map(textures.map((texture) => [texture.hash, texture]));
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

/** Set one instance's matrix to a uniform scale (0 hides a marker). */
function setInstanceScale(
  mesh: THREE.InstancedMesh,
  instances: readonly number[],
  entityIndex: number,
  scaleValue: number,
): void {
  const i = instances.indexOf(entityIndex);
  if (i < 0) return;
  mesh.setMatrixAt(i, new THREE.Matrix4().makeScale(scaleValue, scaleValue, scaleValue));
  mesh.instanceMatrix.needsUpdate = true;
}

/** Restore one instance's matrix from its placement. */
function setInstanceTransform(
  mesh: THREE.InstancedMesh,
  instances: readonly number[],
  entityIndex: number,
  placement: Placed,
): void {
  const i = instances.indexOf(entityIndex);
  if (i < 0) return;
  const matrix = new THREE.Matrix4();
  const position = new THREE.Vector3(placement.position[0], placement.position[1], placement.position[2]);
  const quaternion = new THREE.Quaternion(
    placement.rotation[0],
    placement.rotation[1],
    placement.rotation[2],
    placement.rotation[3],
  );
  const [sx, sy, sz] = placementScale(placement.scaleXy, placement.scaleZ);
  matrix.compose(position, quaternion, new THREE.Vector3(sx, sy, sz));
  mesh.setMatrixAt(i, matrix);
  mesh.instanceMatrix.needsUpdate = true;
}

/** Lay a live edit onto one instance's matrix. */
function setInstanceEdit(
  mesh: THREE.InstancedMesh,
  instances: readonly number[],
  entityIndex: number,
  edit: TransformEdit,
): void {
  const i = instances.indexOf(entityIndex);
  if (i < 0) return;
  const matrix = new THREE.Matrix4();
  const position = new THREE.Vector3(edit.position[0], edit.position[1], edit.position[2]);
  const quaternion = new THREE.Quaternion(
    edit.rotation[0],
    edit.rotation[1],
    edit.rotation[2],
    edit.rotation[3],
  );
  const [sx, sy, sz] = placementScale(edit.scaleXy, edit.scaleZ);
  matrix.compose(position, quaternion, new THREE.Vector3(sx, sy, sz));
  mesh.setMatrixAt(i, matrix);
  mesh.instanceMatrix.needsUpdate = true;
}

/** Dispose every geometry and material under an object. */
function disposeObject3D(root: THREE.Object3D): void {
  root.traverse((object) => {
    const mesh = object as THREE.Mesh;
    if (mesh.geometry) mesh.geometry.dispose();
    const material = mesh.material as THREE.Material | THREE.Material[] | undefined;
    if (Array.isArray(material)) {
      for (const m of material) m.dispose();
    } else if (material) {
      material.dispose();
    }
  });
  while (root.children.length > 0) {
    root.remove(root.children[0]);
  }
}

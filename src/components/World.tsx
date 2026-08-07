import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import * as THREE from "three";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  ChevronRight,
  Crosshair,
  FilePlus,
  Globe2,
  Layers,
  Loader2,
  MapPin,
  X,
} from "lucide-react";
import { formatHash } from "../lib/format";

import { reportGpuCaps } from "../lib/gpu";
import { createGeometryStore } from "../lib/world/geometryStore";
import {
  createPlacementIndex,
  type PlacementIndex,
} from "../lib/world/placementIndex";
import { preloadableCount, preloadTargets } from "../lib/world/preload";
import { BakePanel } from "./world/BakePanel";
import {
  applyEdit,
  axisRotate,
  boundsOf,
  clearWorldCaches,
  colorForLod,
  commitHistory,
  demoWorld,
  editOf,
  gtaToWorld,
  keyFor,
  lodBandForDistance,
  parseKey,
  parseProject,
  placementScale,
  projectLoad,
  projectSave,
  serializeProject,
  streamingDecision,
  worldAsset,
  worldDlcYmaps,
  worldMap,
  worldMapFile,
  worldMapsInGroup,
  worldModel,
  worldStatus,
  worldToGta,
  type ContentGroup,
  type GeometryLoad,
  type HistoryEntry,
  type Placed,
  type RegionEntry,
  type WorldModel,
  type TransformEdit,
  type WorldMap,
  type WorldStatus,
  type YmapListingPage,
} from "../lib/world";

/** Placing a map whose LOD buckets would exceed this turns shadows off. */
const SHADOW_LIMIT = 5_000;
/** How many `.ymap` rows a group's expanded list shows. */
const MAX_YMAP_ROWS = 40;
/** How many entities one streaming pass may start loading. */
const STREAM_BATCH = 24;
/** How many base-game maps one auto-load chunk reads at once. */
const LOAD_BATCH = 16;
/** How often the streaming pass re-decides what is loaded. */
const STREAM_INTERVAL_MS = 250;
/** How many entities a preload starts at once. */
const PRELOAD_BATCH = 12;
/**
 * How many entities a preload will load before it stops.
 *
 * The whole base game is around 693,000 placements, and one `THREE.Group` of
 * meshes per entity does not survive anything near that. Until archetypes are
 * instanced, this cap is what stands between "load the map" and a locked-up
 * renderer, so it is deliberately low enough to be safe on a modest GPU and
 * raisable by anyone who knows what their machine can take.
 */
const PRELOAD_CAP = 5_000;

/**
 * Session-wide caches, outside the component so they survive tab switches:
 * re-entering World re-streams without re-resolving archetype names or
 * re-decoding model geometry.
 */
const assetCache = new Map<string, string | null>();
const modelCache = new Map<string, WorldModel>();
/**
 * Loaded geometry, outside React for the reasons in `geometryStore.ts`: keeping
 * it in `useState` made every load O(n) three times over, which streaming hid
 * and preloading does not.
 */
const geometryStore = createGeometryStore();
/** The mounted game folder, so the session caches drop when it changes. */
let lastFolder: string | null = null;

export type Selection = {
  readonly mapKey: string;
  readonly entityIndex: number;
};

/** Which gizmo the selected entity gets. */
export type GizmoMode = "move" | "rotate" | "scale";

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
  /** A load with a known count, for the progress bar. */
  const [progress, setProgress] = useState<{
    done: number;
    total: number;
    label: string;
  } | null>(null);
  const [selected, setSelected] = useState<Selection | null>(null);
  // Entities whose real geometry is loaded, streamed or preloaded. The version
  // this returns is only a change signal — the array itself comes from the
  // store's own cache, so its identity holds until something actually loads.
  useSyncExternalStore(geometryStore.subscribe, geometryStore.getVersion);
  const geometry = geometryStore.all();
  const [streaming, setStreaming] = useState(false);
  /** Set while a preload runs, so the button can offer to stop it. */
  const [preloading, setPreloading] = useState(false);
  const preloadCancel = useRef(false);
  /** In-progress transform edits, applied live by the canvas, per entity key. */
  const [edits, setEdits] = useState<ReadonlyMap<string, TransformEdit>>(new Map());
  const [undo, setUndo] = useState<readonly HistoryEntry[]>([]);
  const [redo, setRedo] = useState<readonly HistoryEntry[]>([]);
  const [gizmoMode, setGizmoMode] = useState<GizmoMode>("move");

  // Asset and model reads are cached so that a street of a hundred identical
  // streetlamps resolves and loads its model once, not a hundred times. The
  // caches are module-level so they survive a tab switch: re-entering World
  // re-streams without re-resolving names or re-decoding geometry.
  const pending = useRef(new Set<string>());
  /** The camera's world position, written by the canvas every frame. */
  const cameraRef = useRef<{ position: [number, number, number] | null }>({ position: null });
  /** Base maps are loaded once per mount, not every time status re-resolves. */
  const autoLoadedBase = useRef(false);

  useEffect(() => {
    let cancelled = false;
    worldStatus()
      .then((loaded) => {
        if (cancelled) return;
        if (loaded.folder !== lastFolder) {
          lastFolder = loaded.folder;
          assetCache.clear();
          modelCache.clear();
          geometryStore.clear();
          clearWorldCaches();
        }
        setStatus(loaded);
        setError(null);
        // A game folder means real geometry: stream it from the start so the
        // world reads like the engine's — fly and the city builds around you.
        // The demo plaza has no assets to stream, so it stays off there.
        if (loaded.gameOpen) setStreaming(true);
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

  /** Load every base-game map, the world the install ships. Runs once per
   *  mount so the world is never an empty grid; the reads are batched and the
   *  canvas rebuilds once, not per map. */
  const loadBaseMaps = useCallback(async () => {
    setBusy(true);
    try {
      const maps = await worldMapsInGroup("base");
      const all: WorldMap[] = [];
      setProgress({ done: 0, total: maps.length, label: "Loading base maps" });
      // Chunks bound concurrency so hundreds of parses do not all start at
      // once; the single `setPlaced` below means the canvas rebuilds once.
      for (let i = 0; i < maps.length; i += LOAD_BATCH) {
        const chunk = maps.slice(i, i + LOAD_BATCH);
        // oxlint-disable-next-line no-await-in-loop
        const results = await Promise.allSettled(chunk.map((m) => worldMap(m.path)));
        const done = results.filter((r) => r.status === "fulfilled").length;
        for (const result of results) {
          if (result.status === "fulfilled") all.push(result.value);
        }
        setProgress((current) =>
          current ? { ...current, done: current.done + done } : current,
        );
      }
      // Deduplicated through a set, not `next.some(...)`: the base group is
      // 4,601 maps, and a linear scan per map is ten million path comparisons
      // to add a list that is already distinct.
      setPlaced((current) => {
        const seen = new Set(current.map((m) => m.path));
        const next = [...current];
        for (const map of all) {
          if (seen.has(map.path)) continue;
          seen.add(map.path);
          next.push(map);
        }
        return next;
      });
      setError(null);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }, []);

  /** Place a `.ymap` the user picked from disk — custom or mod maps. */
  const loadCustomYmap = useCallback(async () => {
    const path = await openDialog({
      filters: [{ name: "GTA V maps", extensions: ["ymap"] }],
      title: "Load a custom YMAP",
    });
    if (typeof path !== "string") return;
    setBusy(true);
    try {
      const map = await worldMapFile(path);
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

  // A game folder is open: fill the world with the base game maps, so entering
  // World is never an empty grid. Once per mount; the demo stays for no-folder.
  useEffect(() => {
    if (!status?.gameOpen || autoLoadedBase.current) return;
    autoLoadedBase.current = true;
    void loadBaseMaps();
  }, [status?.gameOpen, loadBaseMaps]);

  const remove = useCallback((path: string) => {
    setPlaced((current) => current.filter((m) => m.path !== path));
    geometryStore.removeMap(path);
    setSelected((current) => (current && current.mapKey === path ? null : current));
  }, []);

  /** Load one entity's real geometry, resolving its archetype to a model. */
  // Deps are empty and must stay that way: the loaded set is read from the
  // store rather than closed over, so this callback is stable and the streaming
  // interval that depends on it is not torn down every time a model arrives.
  const loadEntity = useCallback(async (mapKey: string, entity: Placed) => {
    const key = keyFor(mapKey, entity.index);
    if (geometryStore.has(key) || pending.current.has(key)) return;
    // A name the table could not resolve has no file to load.
    if (entity.name.startsWith("hash_")) return;

    pending.current.add(key);
    try {
      let modelPath = assetCache.get(entity.name);
      if (modelPath === undefined) {
        modelPath = await worldAsset(entity.name);
        assetCache.set(entity.name, modelPath);
      }
      if (!modelPath) return;
      let model = modelCache.get(modelPath);
      if (!model) {
        // Awaited here rather than in an effect so the ordering holds by
        // construction: no model is ever asked for before the backend knows
        // which block formats this context can take. Memoized, so every call
        // after the first is one already-resolved promise.
        await reportGpuCaps();
        model = await worldModel(modelPath);
        modelCache.set(modelPath, model);
      }
      geometryStore.add({ mapKey, entity, modelPath, model });
    } catch (cause) {
      setError(String(cause));
    } finally {
      pending.current.delete(key);
    }
  }, []);

  const unloadEntity = useCallback((mapKey: string, entityIndex: number) => {
    geometryStore.remove(keyFor(mapKey, entityIndex));
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

  const saveProject = useCallback(async () => {
    const path = await saveDialog({
      defaultPath: "world.cwproj",
      filters: [{ name: "AstraMapper world project", extensions: ["cwproj"] }],
    });
    if (typeof path !== "string") return;
    setBusy(true);
    try {
      await projectSave(path, serializeProject(placed.map((m) => m.path), edits));
      setError(null);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, [placed, edits]);

  const loadProject = useCallback(async () => {
    const path = await openDialog({
      filters: [{ name: "AstraMapper world project", extensions: ["cwproj"] }],
    });
    if (typeof path !== "string") return;
    setBusy(true);
    try {
      const parsed = parseProject(await projectLoad(path));
      // Re-place every map the project names, then layer the edits back on.
      const loaded: WorldMap[] = [];
      setProgress({ done: 0, total: parsed.maps.length, label: "Loading project" });
      for (let i = 0; i < parsed.maps.length; i += LOAD_BATCH) {
        const chunk = parsed.maps.slice(i, i + LOAD_BATCH);
        // oxlint-disable-next-line no-await-in-loop
        const results = await Promise.allSettled(chunk.map((mapPath) => worldMap(mapPath)));
        const done = results.filter((r) => r.status === "fulfilled").length;
        for (const result of results) {
          if (result.status === "fulfilled") loaded.push(result.value);
        }
        setProgress((current) =>
          current ? { ...current, done: current.done + done } : current,
        );
      }
      const failed = parsed.maps.length - loaded.length;
      setPlaced(loaded);
      setEdits(new Map(parsed.edits.map((e) => [e.key, e.edit])));
      geometryStore.clear();
      setUndo([]);
      setRedo([]);
      setSelected(null);
      setError(
        failed > 0 ? `${failed} of ${parsed.maps.length} maps could not be loaded` : null,
      );
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }, []);

  const newProject = useCallback(() => {
    setPlaced([]);
    setEdits(new Map());
    geometryStore.clear();
    setUndo([]);
    setRedo([]);
    setSelected(null);
    setError(null);
  }, []);

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

  /**
   * Constant-time `(mapKey, entityIndex)` → placement, built once per placement
   * change and shared by everything that resolves a selection, a raycast hit or
   * a streaming candidate. See `placementIndex.ts` for why the scans it
   * replaces could not stay: the ones below run per frame while a gizmo drags.
   */
  const index = useMemo(() => createPlacementIndex(entries), [entries]);

  const selectedPlacement: { entry: (typeof entries)[number]; placement: Placed } | null =
    useMemo(() => {
      if (!selected) return null;
      const entry = index.entry(selected.mapKey);
      if (!entry) return null;
      const placement = index.placement(selected.mapKey, selected.entityIndex);
      if (!placement) return null;
      return {
        entry,
        placement: applyEdit(placement, edits.get(keyFor(selected.mapKey, selected.entityIndex))),
      };
    }, [index, selected, edits]);

  // The scheduler's view of the placed maps: each one's declared streaming box
  // and its entities. The demo has no assets to stream, so it stays out.
  const regionEntries: readonly RegionEntry[] = useMemo(
    () =>
      placed.map((map) => ({
        key: map.path,
        extentsMin: map.extentsMin,
        extentsMax: map.extentsMax,
        placements: map.entities,
      })),
    [placed],
  );

  // The streaming pass: every interval, decide what to load and unload from the
  // camera's position. The decision is pure (`streamingDecision`); the effect
  // just carries it out against the load caches and geometry state.
  // The loaded set is read from the store inside the pass, so `geometry` is not
  // a dependency and this interval survives every load rather than being torn
  // down and recreated by each one.
  useEffect(() => {
    if (!streaming || !status?.gameOpen) return;
    const interval = window.setInterval(async () => {
      const cam = cameraRef.current.position;
      if (!cam || regionEntries.length === 0) return;
      const cameraGta = worldToGta(cam);
      const loadedKeys = new Set(
        geometryStore.all().map((g) => keyFor(g.mapKey, g.entity.index)),
      );
      const { load, unload } = streamingDecision(
        cameraGta,
        regionEntries,
        loadedKeys,
        pending.current,
        STREAM_BATCH,
      );
      for (const candidate of load) {
        const entity = index.placement(candidate.mapKey, candidate.entityIndex);
        if (entity) void loadEntity(candidate.mapKey, entity);
      }
      geometryStore.removeMany(unload);
    }, STREAM_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, [streaming, status?.gameOpen, regionEntries, index, loadEntity]);

  /** Every entity a preload could reach. Memoized because it walks every
   *  placement of every placed map, which is six figures with the base game. */
  const preloadable = useMemo(() => preloadableCount(regionEntries), [regionEntries]);

  const stopPreload = useCallback(() => {
    preloadCancel.current = true;
  }, []);

  /**
   * Load everything in the placed maps at once, instead of by distance.
   *
   * Streaming is switched off first, and deliberately: its unload rule drops
   * anything past its radius, so leaving it on would have it tearing out what
   * this just loaded as soon as the camera moved.
   */
  const preloadAll = useCallback(
    async (cap: number) => {
      setStreaming(false);
      setPreloading(true);
      preloadCancel.current = false;
      try {
        const cam = cameraRef.current.position;
        const excluded = new Set<string>(pending.current);
        for (const load of geometryStore.all()) {
          excluded.add(keyFor(load.mapKey, load.entity.index));
        }
        const targets = preloadTargets(
          regionEntries,
          excluded,
          cam ? worldToGta(cam) : null,
          cap,
        );
        setProgress({ done: 0, total: targets.length, label: "Preloading placed maps" });

        for (let at = 0; at < targets.length; at += PRELOAD_BATCH) {
          if (preloadCancel.current) break;
          const batch = targets.slice(at, at + PRELOAD_BATCH);
          // oxlint-disable-next-line no-await-in-loop
          await Promise.allSettled(batch.map((t) => loadEntity(t.mapKey, t.entity)));
          setProgress((current) =>
            current ? { ...current, done: current.done + batch.length } : current,
          );
        }
        setError(null);
      } catch (cause) {
        setError(String(cause));
      } finally {
        setProgress(null);
        setPreloading(false);
        preloadCancel.current = false;
      }
    },
    [regionEntries, loadEntity],
  );

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

      {progress && (
        <div className="shrink-0 border-b border-edge bg-surface-1 px-4 py-1.5 text-xs text-ink-dim">
          <div className="flex items-center justify-between gap-3">
            <span>
              {progress.label} · {progress.done.toLocaleString()} of{" "}
              {progress.total.toLocaleString()} maps
            </span>
            <span>{Math.round((progress.done / Math.max(progress.total, 1)) * 100)}%</span>
          </div>
          <div className="mt-1 h-1 overflow-hidden rounded bg-surface-2">
            <div
              className="h-full rounded bg-accent transition-[width] duration-150"
              style={{
                width: `${(progress.done / Math.max(progress.total, 1)) * 100}%`,
              }}
            />
          </div>
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <aside className="flex w-80 shrink-0 flex-col overflow-y-auto border-r border-edge bg-surface-1">
          {!status ? (
            <div className="flex items-center gap-2 p-4 text-xs text-ink-dim">
              <Loader2 size={13} className="animate-spin" /> reading world status…
            </div>
          ) : (
            <>
              <Section title="Project">
                <div className="flex items-center gap-2 px-4 py-1">
                  <button
                    type="button"
                    onClick={() => void saveProject()}
                    disabled={busy}
                    className="rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
                  >
                    Save
                  </button>
                  <button
                    type="button"
                    onClick={() => void loadProject()}
                    disabled={busy}
                    className="rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
                  >
                    Load
                  </button>
                  <button
                    type="button"
                    onClick={newProject}
                    disabled={busy}
                    className="rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
                  >
                    New
                  </button>
                </div>
                <p className="px-4 pb-1 text-[11px] text-ink-dim">
                  A .cwproj stores the placed maps and your edits, to load in
                  another session.
                </p>
              </Section>

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
                    <button
                      type="button"
                      onClick={() => void loadCustomYmap()}
                      disabled={busy}
                      className="mx-4 mt-2 flex items-center gap-1.5 rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-50"
                    >
                      <FilePlus size={13} strokeWidth={1.75} />
                      Load custom YMAP…
                    </button>
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

                  <Section title="Preload">
                    <PreloadPanel
                      placedEntities={preloadable}
                      loaded={geometry.length}
                      running={preloading}
                      busy={busy}
                      onStart={preloadAll}
                      onStop={stopPreload}
                    />
                  </Section>

                  <Section title="Asset cache">
                    <BakePanel groups={[...checkedGroups]} />
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
                    {selectedPlacement.placement.fromMlo && (
                      <span className="rounded bg-surface-2 px-1 py-px text-[10px] text-ink-dim">
                        interior entity
                      </span>
                    )}
                    <div className="flex items-center gap-1 pt-1">
                      {(["move", "rotate", "scale"] as const).map((mode) => (
                        <button
                          key={mode}
                          type="button"
                          onClick={() => setGizmoMode(mode)}
                          className={`rounded px-2 py-1 text-xs transition-colors hover:bg-surface-2 ${
                            gizmoMode === mode
                              ? "bg-surface-2 text-accent"
                              : "text-ink-dim hover:text-ink"
                          }`}
                        >
                          {mode}
                        </button>
                      ))}
                      {edits.has(keyFor(selectedPlacement.entry.key, selectedPlacement.placement.index)) && (
                        <span className="ml-auto rounded bg-surface-2 px-1 py-px text-[10px] text-ink-dim">
                          edited
                        </span>
                      )}
                    </div>
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
          index={index}
          geometry={geometry}
          cameraRef={cameraRef}
          edits={edits}
          onEdit={onEdit}
          onEditCommit={onEditCommit}
          gizmoMode={gizmoMode}
          selected={selected}
          onSelect={setSelected}
          shadowed={totalPlaced <= SHADOW_LIMIT}
          busy={busy}
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

/**
 * Load the placed maps in full, rather than by camera distance.
 *
 * The cap is exposed rather than hidden because it is the only thing bounding
 * the load, and the right value depends on the GPU. It is shown against the
 * real number of placed entities so raising it is an informed choice: with the
 * base game placed that number is six figures, and nothing near it will render
 * until archetypes are instanced.
 */
function PreloadPanel({
  placedEntities,
  loaded,
  running,
  busy,
  onStart,
  onStop,
}: {
  placedEntities: number;
  loaded: number;
  running: boolean;
  busy: boolean;
  onStart: (cap: number) => void;
  onStop: () => void;
}): React.JSX.Element {
  const [cap, setCap] = useState(PRELOAD_CAP);
  const reachable = Math.min(cap, placedEntities);

  return (
    <div className="px-4 pb-1 text-xs">
      <p className="pb-2 text-[11px] text-ink-dim">
        Loads everything in the placed maps at once instead of streaming it by
        distance. Turns streaming off, because its unload rule would drop what
        this loads as soon as you moved.
      </p>

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={running ? onStop : () => onStart(cap)}
          disabled={busy || placedEntities === 0}
          className="flex items-center gap-1.5 rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
        >
          {running ? <Loader2 size={12} className="animate-spin" /> : <Layers size={12} />}
          {running ? "Stop" : "Preload"}
        </button>
        <label className="ml-auto flex items-center gap-1.5 text-[11px] text-ink-dim">
          cap
          <input
            type="number"
            min={1}
            step={500}
            value={cap}
            disabled={running}
            onChange={(event) => setCap(Math.max(1, Number(event.target.value) || 1))}
            className="w-20 rounded bg-surface-2 px-1.5 py-0.5 text-right font-mono text-ink outline-none disabled:opacity-40"
          />
        </label>
      </div>

      <p className="pt-2 text-[11px] text-ink-dim">
        {loaded.toLocaleString()} loaded of {placedEntities.toLocaleString()} placed
        {placedEntities > cap && `, capped at ${reachable.toLocaleString()}`}
      </p>
    </div>
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
  /** `entries`, keyed for constant-time lookup. Built by the parent so the
   *  canvas, the selection panel and the streamer all read one index. */
  index: PlacementIndex<CanvasEntry>;
  geometry: readonly GeometryLoad[];
  cameraRef: { readonly current: { position: [number, number, number] | null } };
  edits: ReadonlyMap<string, TransformEdit>;
  onEdit: (key: string, edit: TransformEdit) => void;
  onEditCommit: (key: string, before: TransformEdit, after: TransformEdit) => void;
  gizmoMode: GizmoMode;
  selected: Selection | null;
  onSelect: (selection: Selection | null) => void;
  shadowed: boolean;
  /** A world-level load is running; hides the "empty" hint while it does. */
  busy: boolean;
};

/** A hit produced by the raycast, tied back to the placement it landed on. */
type Hit = {
  readonly mapKey: string;
  readonly entityIndex: number;
  readonly distance: number;
};

/**
 * Where one entity's marker box lives: which instanced mesh, and which slot.
 *
 * Boxes are bucketed by LOD colour, so a map's placements are spread across
 * several `InstancedMesh` objects. Resolving an entity used to be
 * `instances.indexOf(entityIndex)` against **the first bucket only**, which was
 * both a linear scan per call and wrong for every entity in a later bucket —
 * their markers never stepped aside for a loaded model and never followed an
 * edit. Keying every bucket by entity index fixes both at once.
 */
type BoxSlot = {
  readonly mesh: THREE.InstancedMesh;
  readonly slot: number;
};

/** The LOD-swap state for one loaded entity. */
type LoadedGeometry = {
  readonly holder: THREE.Group;
  readonly lodGroups: readonly THREE.Mesh[][];
  readonly lodDist: number;
  readonly positionGta: readonly [number, number, number];
  /** Which entity this is, so a raycast hit and an unload can name it without
   *  taking the key apart — a map key is a path and may contain the separator. */
  readonly mapKey: string;
  readonly entityIndex: number;
  /**
   * The LOD bundle currently visible, or -1 before the first pass.
   *
   * Kept so the per-frame swap only touches meshes when the band actually
   * changes. Writing `visible` on every mesh of every bundle of every loaded
   * entity, every frame, is hundreds of thousands of property writes a second
   * once a preload has filled the world — for a value that is almost always
   * the one already there.
   */
  band: number;
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
  index,
  geometry,
  cameraRef,
  edits,
  onEdit,
  onEditCommit,
  gizmoMode,
  selected,
  onSelect,
  shadowed,
  busy,
}: CanvasProps): React.JSX.Element {
  const host = useRef<HTMLDivElement>(null);
  const indexRef = useRef(index);
  indexRef.current = index;
  const editsRef = useRef(edits);
  editsRef.current = edits;
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const gizmoModeRef = useRef(gizmoMode);
  gizmoModeRef.current = gizmoMode;

  const worldGroup = useRef<THREE.Group | null>(null);
  const geometryGroup = useRef<THREE.Group | null>(null);
  const highlight = useRef<THREE.LineSegments | null>(null);
  /** Map key → entity index → the instanced mesh and slot holding its marker. */
  const boxByMap = useRef(new Map<string, Map<number, BoxSlot>>());
  const loadedState = useRef(new Map<string, LoadedGeometry>());
  const lodUpdate = useRef<((cameraWorld: THREE.Vector3) => void) | null>(null);
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

    // The placed content decides how big the grid is: a map a few thousand
    // units across still sits on a grid that covers it. The cell size stays
    // constant (10 units) while the total grows with the content's radius.
    const { center, radius } = boundsOf(entries.flatMap((entry) => entry.placements));
    const gridSize = Math.max(400, Math.ceil((radius * 2.4) / 100) * 100);
    const grid = new THREE.GridHelper(
      gridSize,
      Math.round(gridSize / 10),
      0x3a4250,
      0x2a303a,
    );
    // Centre the grid on the content's horizontal centre (the GTA ground
    // plane is Z = 0, so the world-space Y stays at the grid's default).
    const [gridX, , gridZ] = gtaToWorld(center);
    grid.position.set(gridX, 0, gridZ);
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
    const boxes = new Map<string, Map<number, BoxSlot>>();

    // One InstancedMesh per map-and-colour bucket. Three.js gives instanced
    // meshes a single material, so buckets are how the LOD colours show; the
    // parallel `indexes` list maps each instance back to its entity for the
    // raycast, and `boxes` maps it the other way for the marker updates.
    for (const entry of entries) {
      const byColour = new Map<number, Placed[]>();
      for (const placement of entry.placements) {
        const colour = colorForLod(placement.lodLevel, placement.isMlo);
        const bucket = byColour.get(colour) ?? [];
        bucket.push(placement);
        byColour.set(colour, bucket);
      }
      const slots = new Map<number, BoxSlot>();
      boxes.set(entry.key, slots);
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
          slots.set(p.index, { mesh, slot: i });
        }
        mesh.instanceMatrix.needsUpdate = true;
        mesh.castShadow = shadowed;
        mesh.receiveShadow = shadowed;
        mesh.computeBoundingSphere();
        world.add(mesh);
        boxMeshes.push({ mesh, mapKey: entry.key, indexes });
      }
    }
    boxByMap.current = boxes;

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
    const gizmoLines: THREE.Line[] = [];
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
      gizmoLines.push(line);
    }

    // Rotate rings: a visual torus per axis, plus a fat invisible twin that the
    // raycaster can actually hit.
    const rotateRings: THREE.Mesh[] = [];
    const rotateTori: { mesh: THREE.Mesh; axis: number }[] = [];
    const orientRing = (mesh: THREE.Mesh, dir: readonly [number, number, number]): void => {
      if (dir[0] === 1) mesh.rotation.y = Math.PI / 2;
      else if (dir[1] === 1) mesh.rotation.x = Math.PI / 2;
    };
    for (let i = 0; i < GIZMO_AXES.length; i += 1) {
      const axis = GIZMO_AXES[i];
      const ring = new THREE.Mesh(
        new THREE.TorusGeometry(GIZMO_LENGTH * 0.7, 0.3, 8, 48),
        new THREE.MeshBasicMaterial({
          color: axis.color,
          side: THREE.DoubleSide,
          transparent: true,
          opacity: 0.85,
        }),
      );
      const pickRing = new THREE.Mesh(
        new THREE.TorusGeometry(GIZMO_LENGTH * 0.7, 1.8, 8, 48),
        new THREE.MeshBasicMaterial({ transparent: true, opacity: 0 }),
      );
      orientRing(ring, axis.dirGta);
      orientRing(pickRing, axis.dirGta);
      gizmo.add(ring);
      gizmo.add(pickRing);
      rotateRings.push(ring);
      rotateTori.push({ mesh: pickRing, axis: i });
    }
    gizmoOriginWorld.current = null;

    // Frame the camera over everything that was placed. A full-map world
    // (thousands of units across) starts mid-air over its centre so the
    // streamer has real geometry in view immediately; small worlds keep the
    // overview framing.
    const [fx, fy, fz] = gtaToWorld(center);
    const framed = new THREE.Vector3(fx, fy, fz);
    if (radius > 1500) {
      const startHeight = Math.min(radius * 0.15, 600);
      camera.position.set(
        framed.x + startHeight * 0.3,
        framed.y + startHeight,
        framed.z + startHeight * 0.3,
      );
    } else {
      const offset = new THREE.Vector3(1, 0.7, 1).normalize().multiplyScalar(radius * 1.6);
      camera.position.copy(framed).add(offset);
    }
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
      dragKind: "" as "" | "move" | "scale",
      gizmoKey: "",
      gizmoStart: null as TransformEdit | null,
      gizmoCurrent: null as TransformEdit | null,
      gizmoPlane: null as THREE.Plane | null,
      gizmoLastPoint: null as THREE.Vector3 | null,
      gizmoMoved: false,
      // A rotate-ring drag, when one is live.
      rotateAxis: -1,
      rotateStart: null as TransformEdit | null,
      rotateCurrent: null as TransformEdit | null,
      rotatePlane: null as THREE.Plane | null,
      rotateCenter: null as THREE.Vector3 | null,
      rotateBasisU: null as THREE.Vector3 | null,
      rotateBasisV: null as THREE.Vector3 | null,
      rotateTotal: 0,
      rotateLastAngle: 0,
      rotateMoved: false,
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
      const target = parseKey(key);
      if (!target) return null;
      const placement = indexRef.current.placement(target.mapKey, target.entityIndex);
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

    /** Which rotate ring is under the pointer, by raycast. */
    const pickRotateAxis = (clientX: number, clientY: number): number => {
      const rect = renderer.domElement.getBoundingClientRect();
      pointer.x = ((clientX - rect.left) / rect.width) * 2 - 1;
      pointer.y = -((clientY - rect.top) / rect.height) * 2 + 1;
      raycaster.setFromCamera(pointer, camera);
      for (const { mesh, axis } of rotateTori) {
        if (raycaster.intersectObject(mesh, false).length > 0) return axis;
      }
      return -1;
    };

    /** The angle of a point on a ring plane, for rotation deltas. */
    const angleAround = (
      point: THREE.Vector3,
      ringCenter: THREE.Vector3,
      u: THREE.Vector3,
      v: THREE.Vector3,
    ): number => {
      const p = point.clone().sub(ringCenter);
      return Math.atan2(p.dot(v), p.dot(u));
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

      // A selected entity owns the left button: grab its gizmo, or fall back to
      // rotating the camera. Rotate grabs a ring, move and scale an axis.
      const selection = selectedRef.current;
      if (selection) {
        const key = keyFor(selection.mapKey, selection.entityIndex);
        if (gizmoModeRef.current === "rotate") {
          const ring = pickRotateAxis(event.clientX, event.clientY);
          if (ring >= 0) {
            const start = effectiveEdit(key);
            if (start) {
              const ringCenter = gizmoOriginWorld.current!;
              state.rotateAxis = ring;
              state.gizmoKey = key;
              state.rotateStart = start;
              state.rotateCurrent = start;
              state.rotateCenter = ringCenter.clone();
              const [wx, wy, wz] = gtaToWorld(GIZMO_AXES[ring].dirGta);
              const axisWorld = new THREE.Vector3(wx, wy, wz);
              state.rotatePlane = new THREE.Plane().setFromNormalAndCoplanarPoint(axisWorld, ringCenter);
              const helper = Math.abs(axisWorld.y) < 0.9 ? new THREE.Vector3(0, 1, 0) : new THREE.Vector3(1, 0, 0);
              const u = new THREE.Vector3().crossVectors(axisWorld, helper).normalize();
              const v = new THREE.Vector3().crossVectors(axisWorld, u).normalize();
              state.rotateBasisU = u;
              state.rotateBasisV = v;
              const point = new THREE.Vector3();
              rayThrough(event.clientX, event.clientY).intersectPlane(state.rotatePlane, point);
              state.rotateLastAngle = angleAround(point, ringCenter, u, v);
              state.rotateMoved = false;
              return;
            }
          }
        } else {
          const axis = pickGizmoAxis(event.clientX, event.clientY);
          if (axis >= 0) {
            const start = effectiveEdit(key);
            if (start) {
              state.gizmoAxis = axis;
              state.dragKind = gizmoModeRef.current === "scale" ? "scale" : "move";
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
      }
      state.dragging = true;
    };

    const onPointerMove = (event: PointerEvent): void => {
      if (state.rotateAxis >= 0) {
        const point = new THREE.Vector3();
        if (
          !state.rotatePlane ||
          !rayThrough(event.clientX, event.clientY).intersectPlane(state.rotatePlane, point)
        ) {
          return;
        }
        state.moved += Math.abs(event.clientX - state.lastX) + Math.abs(event.clientY - state.lastY);
        state.lastX = event.clientX;
        state.lastY = event.clientY;
        const angle = angleAround(point, state.rotateCenter!, state.rotateBasisU!, state.rotateBasisV!);
        let delta = angle - state.rotateLastAngle;
        state.rotateLastAngle = angle;
        // A drag that crosses the angle's seam jumps by a full turn; treat any
        // delta near ±π as the sign of a crossing, not a real spin.
        if (delta > Math.PI) delta -= Math.PI * 2;
        else if (delta < -Math.PI) delta += Math.PI * 2;
        state.rotateTotal += delta;
        state.rotateMoved = state.rotateMoved || Math.abs(delta) > 1e-4;
        const start = state.rotateStart!;
        const rotation = axisRotate(start.rotation, GIZMO_AXES[state.rotateAxis].dirGta, state.rotateTotal);
        const edit: TransformEdit = { ...start, rotation };
        state.rotateCurrent = edit;
        onEdit(state.gizmoKey, edit);
        return;
      }
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
        const delta = point.sub(state.gizmoLastPoint!).dot(new THREE.Vector3(wx, wy, wz));
        state.gizmoLastPoint = point;
        state.gizmoMoved = state.gizmoMoved || Math.abs(delta) > 1e-6;
        const start = state.gizmoStart!;
        if (state.dragKind === "scale") {
          const factor = Math.max(0.01, 1 + delta / GIZMO_LENGTH);
          const edit: TransformEdit = {
            ...start,
            scaleXy: state.gizmoAxis < 2 ? start.scaleXy * factor : start.scaleXy,
            scaleZ: state.gizmoAxis < 2 ? start.scaleZ : start.scaleZ * factor,
          };
          state.gizmoCurrent = edit;
          onEdit(state.gizmoKey, edit);
        } else {
          const edit: TransformEdit = {
            ...start,
            position: [
              start.position[0] + axis.dirGta[0] * delta,
              start.position[1] + axis.dirGta[1] * delta,
              start.position[2] + axis.dirGta[2] * delta,
            ],
          };
          state.gizmoCurrent = edit;
          onEdit(state.gizmoKey, edit);
        }
        return;
      }
      if (!state.dragging) return;
      const dx = event.clientX - state.lastX;
      const dy = event.clientY - state.lastY;
      state.lastX = event.clientX;
      state.lastY = event.clientY;
      state.moved += Math.abs(dx) + Math.abs(dy);
      state.yaw -= dx * 0.005;
      // Dragging down looks down: a screen-y increase lowers the pitch.
      state.pitch = THREE.MathUtils.clamp(state.pitch - dy * 0.005, -1.55, 1.55);
    };

    const onPointerUp = (): void => {
      if (state.rotateAxis >= 0) {
        if (state.rotateMoved && state.rotateStart && state.rotateCurrent) {
          onEditCommit(state.gizmoKey, state.rotateStart, state.rotateCurrent);
        }
        state.dragging = false;
        state.rotateAxis = -1;
        state.rotateStart = null;
        state.rotateCurrent = null;
        state.rotatePlane = null;
        state.rotateCenter = null;
        state.rotateBasisU = null;
        state.rotateBasisV = null;
        state.rotateTotal = 0;
        state.rotateLastAngle = 0;
        state.rotateMoved = false;
        return;
      }
      if (state.gizmoAxis >= 0) {
        if (state.gizmoMoved && state.gizmoStart && state.gizmoCurrent) {
          onEditCommit(state.gizmoKey, state.gizmoStart, state.gizmoCurrent);
        }
        state.dragging = false;
        state.gizmoAxis = -1;
        state.dragKind = "";
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
      // Dolly faster when far from the world, slower up close.
      const zoomScale = THREE.MathUtils.clamp(
        state.position.distanceTo(framed) / 500,
        1,
        20,
      );
      state.position.addScaledVector(look, event.deltaY * 0.05 * zoomScale);
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
      // Straight off the loaded set, and only the LOD bundle on screen. The
      // flat list this replaces had to be rebuilt from a full traverse every
      // time anything loaded; a click is rare enough to pay for itself here.
      // Testing only the visible band is also the correct behaviour — a hidden
      // LOD used to be selectable through the one being drawn.
      for (const loaded of loadedState.current.values()) {
        const meshes = loaded.lodGroups[loaded.band];
        if (!meshes) continue;
        for (const mesh of meshes) {
          const hits = raycaster.intersectObject(mesh, false);
          const first = hits[0];
          if (!first) continue;
          if (!best || first.distance < best.distance) {
            best = {
              mapKey: loaded.mapKey,
              entityIndex: loaded.entityIndex,
              distance: first.distance,
            };
          }
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
      const placement = indexRef.current.placement(hit.mapKey, hit.entityIndex);
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
      // Far from the world, fly fast — a full-map overview is reachable in
      // seconds, not minutes — and taper to walking pace as the camera gets
      // close, so fine positioning still works.
      const speedScale = THREE.MathUtils.clamp(
        state.position.distanceTo(framed) / 300,
        1,
        25,
      );
      const speed = baseSpeed * speedScale * (state.pressed.has("sprint") ? 4 : 1) * dt;
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
          // Not loaded, so the edit rides on the marker box instead. Only this
          // branch needs the key taken apart, and only for entities under edit.
          const target = parseKey(key);
          if (!target) continue;
          setInstanceTransform(boxByMap.current.get(target.mapKey)?.get(target.entityIndex), edit);
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
      const mode = gizmoModeRef.current;
      for (const line of gizmoLines) line.visible = mode === "move" || mode === "scale";
      for (const ring of rotateRings) ring.visible = mode === "rotate";
      for (const { mesh } of rotateTori) mesh.visible = mode === "rotate";
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

  // Streamed geometry: sync only the loads that changed into the geometry
  // group. Building every loaded model's meshes from scratch on each pass made
  // streaming quadratically slower as the loaded set grew — a streamed model
  // must add its meshes, not rebuild the ones already there.
  //
  // The same rule now governs everything this pass touches. A preload notifies
  // once per frame while it fills the world, so any sweep here that is
  // proportional to *all* loaded geometry, or worse to all placements, is
  // quadratic over the preload. Two of them were:
  //
  //   - the flat raycast list, rebuilt by traversing every holder. It is gone;
  //     `pick` walks `loadedState` directly, which it can do because a click is
  //     a user event and not a per-frame cost.
  //   - restoring the marker boxes, which swept every placement of every placed
  //     map — around 693,000 with the base game — and resolved each one with a
  //     linear `indexOf`. Only the entities that unloaded this pass need
  //     restoring, and those are known exactly.
  useEffect(() => {
    const group = geometryGroup.current;
    if (!group) return;

    const cameraWorld = new THREE.Vector3();
    const matrix = new THREE.Matrix4();
    const position = new THREE.Vector3();
    const quaternion = new THREE.Quaternion();
    const scale = new THREE.Vector3();

    const previous = loadedState.current;
    const next = new Map<string, LoadedGeometry>();
    /** How many of `previous` survived, which is how the drop pass is skipped. */
    let carried = 0;

    // One pass over what should be resident, carrying the already-built models
    // through untouched and building only the ones that just arrived. Keying
    // each load once is the point: the two-collection diff this replaces built
    // a key string per model twice per pass, and a pass runs every frame while
    // a preload fills the world.
    for (const load of geometry) {
      const key = keyFor(load.mapKey, load.entity.index);
      const already = previous.get(key);
      if (already) {
        next.set(key, already);
        carried += 1;
        continue;
      }
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
        for (const mesh of meshes) holder.add(mesh);
      }
      group.add(holder);
      next.set(key, {
        holder,
        lodGroups,
        lodDist: load.entity.lodDist,
        positionGta: load.entity.position,
        mapKey: load.mapKey,
        entityIndex: load.entity.index,
        band: -1,
      });

      // The entity now has real geometry; its box marker must step aside. A
      // model that produced no geometry keeps its marker rather than vanishing.
      if (lodGroups.length > 0) {
        setInstanceScale(boxByMap.current.get(load.mapKey)?.get(load.entity.index), 0);
      }
    }

    // Whatever was resident and is not any more: tear it down and give the
    // entity its marker box back. An unload is the only moment a box needs
    // restoring, which is what lets this be proportional to the drops rather
    // than to every placement in the world.
    //
    // Everything carried through means nothing dropped, and a preload — which
    // only ever adds — takes that shortcut on every one of its passes.
    if (carried !== previous.size) {
      for (const [key, loaded] of previous) {
        if (next.has(key)) continue;
        disposeObject3D(loaded.holder);
        group.remove(loaded.holder);
        const placement = indexRef.current.placement(loaded.mapKey, loaded.entityIndex);
        if (placement) {
          setInstanceTransform(
            boxByMap.current.get(loaded.mapKey)?.get(loaded.entityIndex),
            placement,
          );
        }
      }
    }

    loadedState.current = next;
    lodUpdate.current = (cameraPosition) => {
      cameraWorld.copy(cameraPosition);
      const [cx, cy, cz] = worldToGta([cameraWorld.x, cameraWorld.y, cameraWorld.z]);
      for (const loaded of next.values()) {
        const { lodGroups, lodDist, positionGta } = loaded;
        const dist = Math.hypot(
          positionGta[0] - cx,
          positionGta[1] - cy,
          positionGta[2] - cz,
        );
        const band = lodBandForDistance(dist, lodDist, lodGroups.length);
        // A distance check per entity per frame is cheap; writing `visible` on
        // every mesh of every bundle is not, and the band a model is in almost
        // never changes between two frames.
        if (band === loaded.band) continue;
        for (let i = 0; i < lodGroups.length; i += 1) {
          const visible = i === band;
          for (const mesh of lodGroups[i]) mesh.visible = visible;
        }
        loaded.band = band;
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
    // This effect re-runs on every `edits` change, which during a gizmo drag is
    // once per frame — so the lookup has to be constant-time.
    const base = index.placement(selected.mapKey, selected.entityIndex);
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
  }, [selected, edits, index]);

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={host} className="min-h-0 h-full bg-surface-0" />
      {totalCount(entries) === 0 && !busy && (
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
      if (part.positions.length === 0) continue;
      // Every array here is a view over the IPC response, handed to three.js
      // without a copy. `BufferAttribute` uploads it and three.js keeps the
      // reference, which is what keeps the response buffer alive exactly as
      // long as the meshes that need it.
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(part.positions, 3));
      if (part.normals) {
        geometry.setAttribute("normal", new THREE.BufferAttribute(part.normals, 3));
      }
      if (part.uvs) {
        geometry.setAttribute("uv", new THREE.BufferAttribute(part.uvs, 2));
      }
      geometry.setIndex(new THREE.BufferAttribute(part.indices, 1));
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
        // RAGE bump maps are left-handed (DirectX) tangent space: the reference
        // shader pushes green along the bitangent, and three.js expects the
        // OpenGL convention where it points the other way. A negative y on
        // `normalScale` inverts the green channel in the shader, which is the
        // same correction the Explorer's viewer makes on the CPU and the only
        // one available when the pixels are never decoded. Without it bumps
        // read as dents.
        material.normalScale = new THREE.Vector2(1, -1);
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

/** The texture map for one part, resolved against the model's uploaded blocks. */
function resolvePartTextures(
  part: { readonly diffuse: number | null; readonly normal: number | null },
  textures: WorldModel["textures"],
): { diffuse: THREE.Texture | null; normal: THREE.Texture | null } {
  // A texture whose section came back empty counts as unresolved, which the
  // caller already handles by leaving the part flat grey. Better that than a
  // throw from inside the render setup.
  const byHash = new Map(
    textures.filter((texture) => texture.levels.length > 0).map((t) => [t.hash, t]),
  );
  const diffuse = part.diffuse === null ? null : byHash.get(part.diffuse);
  const normal = part.normal === null ? null : byHash.get(part.normal);
  return {
    // Diffuse carries colour and is authored in sRGB; a normal map carries
    // vectors and must stay linear. Getting this backwards washes the whole
    // scene out, and it is set per slot because the texture does not know
    // which it is.
    diffuse: diffuse ? uploadTexture(diffuse, THREE.SRGBColorSpace) : null,
    normal: normal ? uploadTexture(normal, THREE.NoColorSpace) : null,
  };
}

/** The WebGL internal format each wire format name uploads as. */
const BLOCK_FORMATS: Readonly<Record<string, THREE.CompressedPixelFormat>> = {
  // The RGBA variant of DXT1 rather than the RGB one: same blocks, and it
  // honours the 1-bit alpha GTA uses for cutouts.
  bc1: THREE.RGBA_S3TC_DXT1_Format,
  bc2: THREE.RGBA_S3TC_DXT3_Format,
  bc3: THREE.RGBA_S3TC_DXT5_Format,
  bc4: THREE.RED_RGTC1_Format,
  bc5: THREE.RED_GREEN_RGTC2_Format,
  bc7: THREE.RGBA_BPTC_Format,
};

/**
 * Textures uploaded this session, keyed by the hash the parts reference plus
 * the colour space they were uploaded in.
 *
 * The key used to be the entire base64 PNG string, which meant every cache
 * probe hashed a megabyte. The hash is what the parts actually reference, and
 * the colour space is in the key because the same texture can legitimately be
 * bound as both a diffuse and a normal map by two different parts.
 */
const textureCache = new Map<string, THREE.Texture>();

/**
 * Upload one texture's blocks straight to the GPU.
 *
 * `CompressedTexture` hands the mip chain to `compressedTexImage2D` level by
 * level, so nothing here decodes anything — the `Uint8Array`s are views over
 * the IPC response and go to the driver as they arrived.
 *
 * The one subtlety is an incomplete chain. WebGL renders a texture as black if
 * it declares mips that do not run all the way down to 1x1, and a GTA
 * dictionary may store a truncated chain. When the last level is not 1x1 this
 * uploads mip 0 alone with a non-mipmapped filter, which is worse-looking at
 * distance and correct, rather than the alternative.
 */
function uploadTexture(
  texture: WorldModel["textures"][number],
  colorSpace: THREE.ColorSpace,
): THREE.Texture {
  const key = `${texture.hash}|${texture.format}|${colorSpace}`;
  const cached = textureCache.get(key);
  if (cached) return cached;

  const last = texture.mips[texture.levels.length - 1];
  const complete =
    texture.levels.length > 1 && last !== undefined && last.width <= 1 && last.height <= 1;
  const levels = complete ? texture.levels : texture.levels.slice(0, 1);
  const mipmaps = levels.map((data, level) => ({
    data,
    width: texture.mips[level]!.width,
    height: texture.mips[level]!.height,
  }));

  let built: THREE.Texture;
  if (texture.compressed) {
    const format = BLOCK_FORMATS[texture.format];
    if (!format) throw new Error(`${texture.name} is in an unknown block format`);
    built = new THREE.CompressedTexture(
      mipmaps,
      texture.width,
      texture.height,
      format,
      THREE.UnsignedByteType,
    );
  } else {
    // The backend's fallback for a format this context has no extension for.
    // Still no PNG in the path: these are raw RGBA bytes.
    built = new THREE.DataTexture(
      mipmaps[0]!.data,
      mipmaps[0]!.width,
      mipmaps[0]!.height,
      THREE.RGBAFormat,
      THREE.UnsignedByteType,
    );
    built.mipmaps = mipmaps as THREE.Texture["mipmaps"];
  }

  built.colorSpace = colorSpace;
  // GTA authors its textures to tile; clamping them stretches the edge texel
  // across every repeat of a building facade.
  built.wrapS = THREE.RepeatWrapping;
  built.wrapT = THREE.RepeatWrapping;
  built.minFilter = complete ? THREE.LinearMipmapLinearFilter : THREE.LinearFilter;
  built.magFilter = THREE.LinearFilter;
  built.generateMipmaps = false;
  built.needsUpdate = true;

  textureCache.set(key, built);
  return built;
}

/**
 * Scratch objects for the instance-matrix writers.
 *
 * These run per marker per streaming pass, and three.js's own instancing
 * examples compose through one reused matrix for the same reason: allocating a
 * `Matrix4`, a `Vector3` and a `Quaternion` per call hands the collector four
 * objects for every box that moves. Nothing here is re-entrant or async, so one
 * set is safe.
 */
const scratchMatrix = new THREE.Matrix4();
const scratchPosition = new THREE.Vector3();
const scratchQuaternion = new THREE.Quaternion();
const scratchScale = new THREE.Vector3();

/** Set one instance's matrix to a uniform scale (0 hides a marker). */
function setInstanceScale(box: BoxSlot | undefined, scaleValue: number): void {
  if (!box) return;
  box.mesh.setMatrixAt(box.slot, scratchMatrix.makeScale(scaleValue, scaleValue, scaleValue));
  box.mesh.instanceMatrix.needsUpdate = true;
}

/**
 * Write one instance's matrix from a transform — a placement, or the edit
 * overlaying it. Both carry the same four fields, which is why one writer
 * serves the restore path and the live-edit path.
 */
function setInstanceTransform(box: BoxSlot | undefined, transform: TransformEdit): void {
  if (!box) return;
  scratchPosition.set(transform.position[0], transform.position[1], transform.position[2]);
  scratchQuaternion.set(
    transform.rotation[0],
    transform.rotation[1],
    transform.rotation[2],
    transform.rotation[3],
  );
  const [sx, sy, sz] = placementScale(transform.scaleXy, transform.scaleZ);
  scratchScale.set(sx, sy, sz);
  scratchMatrix.compose(scratchPosition, scratchQuaternion, scratchScale);
  box.mesh.setMatrixAt(box.slot, scratchMatrix);
  box.mesh.instanceMatrix.needsUpdate = true;
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

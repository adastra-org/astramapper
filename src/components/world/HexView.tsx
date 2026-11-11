/**
 * The hexagon view: the map cut into cells, seen from above, with the
 * working set around the camera drawn on it.
 *
 * Built to answer two questions before any backend work exists. How many
 * placements would a zone-based loader actually hold — against the whole world
 * the WebView holds today? And where is the density, so the cell size can follow
 * the city and not the desert?
 *
 * Two modes. *Uniform* is one cell size and a number of rings. *Adaptive* is
 * several sizes at once — small cells where the placements crowd, large ones
 * where they do not — and a placement budget instead of a ring count.
 *
 * Everything is derived from the placements already in memory; nothing is
 * stored or sent. The maths is in `src/lib/world/hex*.ts` and tested there, and
 * the data comes from `useHexModel`, shared with the overlay in the 3D scene.
 */
import { Crosshair, Maximize2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { worldToGta } from "../../lib/world";
import {
  hexCenter,
  hexDiskSize,
  hexDistance,
  hexDisk,
  hexKey,
  hexStats,
  worldToHex,
  type HexStats,
} from "../../lib/world/hex";
import { adaptiveLeafAt, adaptiveWorkingSet, type AdaptiveCell } from "../../lib/world/hexAdaptive";
import { type HexModel, type HexSettings } from "../../lib/world/hexModel";

const WIDTH = 460;
const HEIGHT = 360;

type Pose = {
  readonly current: {
    position: readonly [number, number, number] | null;
    forward: readonly [number, number, number] | null;
  };
};

type View = { cx: number; cy: number; scale: number };

/** What the stats block shows, whichever mode produced it. */
type Summary = {
  readonly cells: number;
  readonly placements: number;
  readonly maxCount: number;
  readonly setCells: number;
  readonly setPlacements: number;
  readonly setLoaded: number;
  /** Adaptive: how far the budget reaches, in metres. */
  readonly reach: number | null;
};

export function HexView({
  model,
  onSettings,
  cameraRef,
  onClose,
}: {
  model: HexModel;
  onSettings: (next: HexSettings) => void;
  cameraRef: Pose;
  onClose: () => void;
}): React.JSX.Element {
  const { settings } = model;
  const [follow, setFollow] = useState(true);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [hover, setHover] = useState("");

  const canvas = useRef<HTMLCanvasElement | null>(null);
  const view = useRef<View>({ cx: 0, cy: 0, scale: 0.1 });
  const pointer = useRef<{ x: number; y: number } | null>(null);
  const drag = useRef<{ x: number; y: number } | null>(null);
  // The draw loop outlives any one render, so it reads these instead of closing
  // over them.
  const modelRef = useRef(model);
  modelRef.current = model;
  const followRef = useRef(follow);
  followRef.current = follow;

  const reach = useCallback((): number => {
    const s = modelRef.current.settings;
    return s.mode === "uniform"
      ? (s.radius + 1.5) * s.size * Math.sqrt(3) * 2
      : 2 * Math.max(s.fine * 16, 800);
  }, []);

  const followCamera = useCallback(() => {
    view.current.scale = Math.min(WIDTH, HEIGHT) / reach();
    setFollow(true);
  }, [reach]);

  const fitMap = useCallback(() => {
    const m = modelRef.current;
    let minX = Infinity;
    let maxX = -Infinity;
    let minY = Infinity;
    let maxY = -Infinity;
    const take = (x: number, y: number) => {
      minX = Math.min(minX, x);
      maxX = Math.max(maxX, x);
      minY = Math.min(minY, y);
      maxY = Math.max(maxY, y);
    };
    if (m.uniform) {
      for (const cell of m.uniform.values()) {
        if (cell.count > 0) take(...hexCenter(cell, m.settings.size));
      }
    } else if (m.adaptive) {
      for (const cell of m.adaptive.cells) take(cell.x, cell.y);
    }
    if (!Number.isFinite(minX)) return;
    const span = Math.max(maxX - minX, maxY - minY, 500) * 1.12;
    view.current = {
      cx: (minX + maxX) / 2,
      cy: (minY + maxY) / 2,
      scale: Math.min(WIDTH, HEIGHT) / span,
    };
    setFollow(false);
  }, []);

  // Re-zoom to the working set when its shape changes.
  useEffect(() => {
    followCamera();
  }, [settings.mode, settings.size, settings.radius, settings.fine, followCamera]);

  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const dpr = window.devicePixelRatio || 1;
    element.width = WIDTH * dpr;
    element.height = HEIGHT * dpr;
    const ctx = element.getContext("2d");
    if (!ctx) return;

    let tick = 0;
    const draw = () => {
      const m = modelRef.current;
      const s = m.settings;
      const pose = cameraRef.current;
      const cam = pose.position ? worldToGta(pose.position) : null;
      if (followRef.current && cam) {
        view.current.cx = cam[0];
        view.current.cy = cam[1];
      }
      const { cx, cy, scale } = view.current;
      const sx = (x: number) => WIDTH / 2 + (x - cx) * scale;
      const sy = (y: number) => HEIGHT / 2 - (y - cy) * scale;

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.fillStyle = "#0d1015";
      ctx.fillRect(0, 0, WIDTH, HEIGHT);

      /** A hexagon by its centre, pointy-top like `hexCorners`. */
      const path = (x: number, y: number, size: number) => {
        ctx.beginPath();
        for (let i = 0; i < 6; i += 1) {
          const angle = (Math.PI / 180) * (60 * i - 30);
          const px = sx(x + size * Math.cos(angle));
          const py = sy(y + size * Math.sin(angle));
          if (i === 0) ctx.moveTo(px, py);
          else ctx.lineTo(px, py);
        }
        ctx.closePath();
      };
      const visible = (x: number, y: number, size: number) => {
        const margin = size * scale * 1.2;
        const px = sx(x);
        const py = sy(y);
        return px > -margin && px < WIDTH + margin && py > -margin && py < HEIGHT + margin;
      };
      const body = (
        x: number,
        y: number,
        size: number,
        count: number,
        loaded: number,
        logMax: number,
      ) => {
        if (!visible(x, y, size)) return;
        path(x, y, size);
        ctx.fillStyle = `rgba(224, 164, 88, ${0.1 + 0.6 * (Math.log1p(count) / logMax)})`;
        ctx.fill();
        ctx.strokeStyle = "rgba(255,255,255,0.07)";
        ctx.lineWidth = 1;
        ctx.stroke();
        if (loaded > 0) {
          path(x, y, size * 0.55);
          ctx.fillStyle = `rgba(92, 200, 176, ${0.25 + 0.65 * Math.min(1, loaded / count)})`;
          ctx.fill();
        }
      };
      const ringStroke = (shade: number, lineWidth: number) => {
        ctx.strokeStyle = `hsl(35 ${shade === 0 ? 90 : 60}% ${Math.max(35, 82 - shade * 40)}%)`;
        ctx.lineWidth = lineWidth;
        ctx.stroke();
      };

      let next: Summary | null = null;
      let hoverText = "";
      const mouse = pointer.current;
      const mouseWorld = mouse
        ? ([(mouse.x - WIDTH / 2) / scale + cx, -(mouse.y - HEIGHT / 2) / scale + cy] as const)
        : null;

      if (m.uniform) {
        let maxCount = 1;
        for (const cell of m.uniform.values()) if (cell.count > maxCount) maxCount = cell.count;
        const logMax = Math.log1p(maxCount);
        for (const cell of m.uniform.values()) {
          if (cell.count === 0) continue;
          const [x, y] = hexCenter(cell, s.size);
          body(x, y, s.size, cell.count, cell.loaded, logMax);
        }
        if (cam) {
          const center = worldToHex(cam[0], cam[1], s.size);
          for (const hex of hexDisk(center, s.radius)) {
            const [x, y] = hexCenter(hex, s.size);
            if (!visible(x, y, s.size)) continue;
            const ring = hexDistance(center, hex);
            path(x, y, s.size);
            ctx.fillStyle = `hsla(35, 80%, 60%, ${ring === 0 ? 0.22 : 0.05})`;
            ctx.fill();
            ringStroke(ring / Math.max(1, s.radius), ring === 0 ? 2 : 1.25);
          }
          const stats: HexStats = hexStats(m.uniform, center, s.radius);
          next = {
            cells: stats.cells,
            placements: stats.placements,
            maxCount: stats.maxCount,
            setCells: stats.cellsInDisk,
            setPlacements: stats.placementsInDisk,
            setLoaded: stats.loadedInDisk,
            reach: null,
          };
        }
        if (mouseWorld) {
          const hex = worldToHex(mouseWorld[0], mouseWorld[1], s.size);
          const cell = m.uniform.get(hexKey(hex.q, hex.r));
          hoverText =
            `cell ${hex.q},${hex.r} · ${(cell?.count ?? 0).toLocaleString()} placements` +
            ` · ${cell?.loaded ?? 0} loaded` +
            (cam ? ` · ring ${hexDistance(worldToHex(cam[0], cam[1], s.size), hex)}` : "");
        }
      } else if (m.adaptive) {
        const grid = m.adaptive;
        const leaves = grid.cells;
        let maxCount = 1;
        for (const cell of leaves) if (cell.count > maxCount) maxCount = cell.count;
        const logMax = Math.log1p(maxCount);
        // Coarse first, so the small cells of a city sit on top of the large
        // ones around it.
        // oxlint-disable-next-line unicorn/no-array-sort
        const ordered = [...leaves].sort((a, b) => a.level - b.level);
        for (const cell of ordered) body(cell.x, cell.y, cell.size, cell.count, cell.loaded, logMax);

        if (cam) {
          const set = adaptiveWorkingSet(grid, cam[0], cam[1], s.budget);
          let loaded = 0;
          for (const cell of set.cells) {
            loaded += cell.loaded;
            if (!visible(cell.x, cell.y, cell.size)) continue;
            path(cell.x, cell.y, cell.size);
            ctx.fillStyle = "hsla(35, 80%, 60%, 0.07)";
            ctx.fill();
            ringStroke(
              Math.min(1, Math.hypot(cell.x - cam[0], cell.y - cam[1]) / Math.max(set.reach, 1)),
              1.25,
            );
          }
          let placements = 0;
          for (const cell of leaves) placements += cell.count;
          next = {
            cells: leaves.length,
            placements,
            maxCount,
            setCells: set.cells.size,
            setPlacements: set.placements,
            setLoaded: loaded,
            reach: set.reach,
          };
        }
        if (mouseWorld) {
          const leaf: AdaptiveCell | null = adaptiveLeafAt(grid, mouseWorld[0], mouseWorld[1]);
          hoverText = leaf
            ? `${leaf.size * 2} m cell · level ${leaf.level} · ${leaf.count.toLocaleString()} placements · ${leaf.loaded} loaded`
            : "no placements here";
        }
      }

      if (cam) {
        const px = sx(cam[0]);
        const py = sy(cam[1]);
        const forward = pose.forward ? worldToGta(pose.forward) : null;
        ctx.save();
        ctx.translate(px, py);
        if (forward) ctx.rotate(Math.atan2(-forward[1], forward[0]));
        ctx.fillStyle = "#ffffff";
        ctx.beginPath();
        ctx.moveTo(9, 0);
        ctx.lineTo(-6, 5);
        ctx.lineTo(-6, -5);
        ctx.closePath();
        ctx.fill();
        ctx.restore();
      }

      tick += 1;
      if (tick % 3 === 1) {
        setSummary(next);
        setHover(hoverText);
      }
    };

    draw();
    const timer = window.setInterval(draw, 100);
    return () => window.clearInterval(timer);
  }, [cameraRef]);

  // Wheel zoom needs a non-passive listener to stop the page scrolling.
  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const next = view.current.scale * Math.exp(-event.deltaY * 0.0015);
      view.current.scale = Math.min(2, Math.max(0.0035, next));
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, []);

  const set = (patch: Partial<HexSettings>) => onSettings({ ...settings, ...patch });
  const total = summary?.placements ?? 0;
  const inSet = summary?.setPlacements ?? 0;
  return (
    <div className="absolute bottom-3 right-3 z-20 w-[460px] overflow-hidden rounded border border-edge bg-surface-1/95 text-xs text-ink shadow-lg backdrop-blur">
      <div className="flex items-center gap-2 border-b border-edge px-3 py-1.5">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-ink-dim">
          Hex view
        </span>
        <div className="ml-2 flex overflow-hidden rounded border border-edge">
          {(["uniform", "adaptive"] as const).map((mode) => (
            <button
              key={mode}
              type="button"
              onClick={() => set({ mode })}
              className={`px-2 py-0.5 capitalize transition-colors ${
                settings.mode === mode
                  ? "bg-[#e0a458] text-black"
                  : "text-ink-dim hover:bg-surface-2"
              }`}
            >
              {mode}
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={followCamera}
          title="Follow the camera"
          className={`ml-auto rounded p-1 transition-colors hover:bg-surface-2 ${follow ? "text-[#e0a458]" : "text-ink-dim"}`}
        >
          <Crosshair size={13} strokeWidth={1.75} />
        </button>
        <button
          type="button"
          onClick={fitMap}
          title="Fit the whole map"
          className="rounded p-1 text-ink-dim transition-colors hover:bg-surface-2"
        >
          <Maximize2 size={13} strokeWidth={1.75} />
        </button>
        <button
          type="button"
          onClick={onClose}
          title="Close"
          className="rounded p-1 text-ink-dim transition-colors hover:bg-surface-2"
        >
          <X size={13} strokeWidth={1.75} />
        </button>
      </div>

      <canvas
        ref={canvas}
        style={{ width: WIDTH, height: HEIGHT, cursor: drag.current ? "grabbing" : "grab" }}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId);
          drag.current = { x: event.clientX, y: event.clientY };
        }}
        onPointerMove={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          pointer.current = { x: event.clientX - rect.left, y: event.clientY - rect.top };
          const last = drag.current;
          if (!last) return;
          view.current.cx -= (event.clientX - last.x) / view.current.scale;
          view.current.cy += (event.clientY - last.y) / view.current.scale;
          drag.current = { x: event.clientX, y: event.clientY };
          if (followRef.current) setFollow(false);
        }}
        onPointerUp={() => {
          drag.current = null;
        }}
        onPointerLeave={() => {
          pointer.current = null;
          setHover("");
        }}
      />

      <div className="grid gap-y-1 border-t border-edge px-3 py-2">
        {settings.mode === "uniform" ? (
          <>
            <Slider label="Cell size" value={settings.size} min={100} max={1000} step={50} unit=" m" onChange={(size) => set({ size })} />
            <Slider label="Rings" value={settings.radius} min={1} max={8} step={1} onChange={(radius) => set({ radius })} />
          </>
        ) : (
          <>
            <Slider label="Finest cell" value={settings.fine} min={50} max={400} step={25} unit=" m" onChange={(fine) => set({ fine })} />
            <Slider label="Split above" value={settings.threshold} min={250} max={8000} step={250} onChange={(threshold) => set({ threshold })} />
            <Slider label="Budget" value={settings.budget} min={10000} max={200000} step={5000} onChange={(budget) => set({ budget })} />
          </>
        )}
      </div>

      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 border-t border-edge px-3 py-2 tabular-nums">
        <dt className="text-ink-dim">World</dt>
        <dd>
          {(summary?.cells ?? 0).toLocaleString()} cells · {total.toLocaleString()} placements ·
          max {(summary?.maxCount ?? 0).toLocaleString()} in one cell
        </dd>
        <dt className="text-ink-dim">
          {settings.mode === "uniform" ? `Disk (${hexDiskSize(settings.radius)})` : "Working set"}
        </dt>
        <dd>
          {(summary?.setCells ?? 0).toLocaleString()} cells ·{" "}
          <span className="text-[#e0a458]">{inSet.toLocaleString()}</span> placements
          {total > 0 && ` (${((inSet / total) * 100).toFixed(1)}% of the world)`}
          {summary?.reach != null && ` · reaches ${Math.round(summary.reach).toLocaleString()} m`}
        </dd>
        <dt className="text-ink-dim">Resident</dt>
        <dd>
          {(summary?.setLoaded ?? 0).toLocaleString()} of {inSet.toLocaleString()} have geometry
        </dd>
        <dt className="text-ink-dim">Hover</dt>
        <dd className="truncate text-ink-dim">{hover || "—"}</dd>
      </dl>
    </div>
  );
}

function Slider({
  label,
  value,
  min,
  max,
  step,
  unit = "",
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  unit?: string;
  onChange: (value: number) => void;
}): React.JSX.Element {
  return (
    <label className="flex items-center gap-2">
      <span className="w-20 shrink-0 text-ink-dim">{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
        className="min-w-0 flex-1 accent-[#e0a458]"
      />
      <span className="w-16 text-right tabular-nums">
        {value.toLocaleString()}
        {unit}
      </span>
    </label>
  );
}

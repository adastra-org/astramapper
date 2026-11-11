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
 * stored or sent. The maths is in `src/lib/world/hex*.ts` and tested there, the
 * drawing is in `hexDraw.ts`, and the data comes from `useHexModel`, shared with
 * the overlay in the 3D scene.
 */
import { Crosshair, Maximize2, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { worldToGta } from "../../lib/world";
import { hexCenter, hexDiskSize } from "../../lib/world/hex";
import type { HexModel, HexSettings } from "../../lib/world/hexModel";
import { drawHex, HEIGHT, WIDTH, type Summary, type View } from "./hexDraw";

type Pose = {
  readonly current: {
    position: readonly [number, number, number] | null;
    forward: readonly [number, number, number] | null;
  };
};

/** How far the camera-following view looks, so the working set fills the panel. */
function followReach(settings: HexSettings): number {
  return settings.mode === "uniform"
    ? (settings.radius + 1.5) * settings.size * Math.sqrt(3) * 2
    : 2 * Math.max(settings.fine * 16, 800);
}

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
  const view = useRef<View>({ cx: 0, cy: 0, scale: Math.min(WIDTH, HEIGHT) / followReach(settings) });
  const pointer = useRef<{ x: number; y: number } | null>(null);
  const drag = useRef<{ x: number; y: number } | null>(null);
  // The draw loop outlives any one render, so it reads these instead of closing
  // over them. They are written in an effect, not during render, so a render
  // React throws away cannot leave its values behind.
  const modelRef = useRef(model);
  const followRef = useRef(follow);
  useEffect(() => {
    modelRef.current = model;
    followRef.current = follow;
  }, [model, follow]);

  // Zoom to the working set when its shape changes. Only the view moves — a ref,
  // not state — so this costs no extra render.
  useEffect(() => {
    view.current.scale = Math.min(WIDTH, HEIGHT) / followReach(settings);
  }, [settings]);

  const followCamera = useCallback(() => {
    view.current.scale = Math.min(WIDTH, HEIGHT) / followReach(modelRef.current.settings);
    setFollow(true);
  }, []);

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

  useEffect(() => {
    const element = canvas.current;
    const ctx = element?.getContext("2d");
    if (!element || !ctx) return;
    const dpr = window.devicePixelRatio || 1;
    element.width = WIDTH * dpr;
    element.height = HEIGHT * dpr;

    let tick = 0;
    const draw = () => {
      const pose = cameraRef.current;
      const cam = pose.position ? worldToGta(pose.position) : null;
      if (followRef.current && cam) {
        view.current.cx = cam[0];
        view.current.cy = cam[1];
      }
      const { cx, cy, scale } = view.current;
      const mouse = pointer.current;
      const frame = drawHex(
        ctx,
        dpr,
        modelRef.current,
        view.current,
        cam,
        pose.forward ? worldToGta(pose.forward) : null,
        mouse ? [(mouse.x - WIDTH / 2) / scale + cx, -(mouse.y - HEIGHT / 2) / scale + cy] : null,
      );
      tick += 1;
      if (tick % 3 === 1) {
        setSummary(frame.summary);
        setHover(frame.hover);
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

  const endDrag = () => {
    drag.current = null;
  };

  return (
    <div className="absolute bottom-3 right-3 z-20 w-[460px] overflow-hidden rounded border border-edge bg-surface-1/95 text-xs text-ink shadow-lg backdrop-blur">
      <Header
        settings={settings}
        follow={follow}
        onSettings={onSettings}
        onFollow={followCamera}
        onFit={fitMap}
        onClose={onClose}
      />
      <canvas
        ref={canvas}
        style={{ width: WIDTH, height: HEIGHT, cursor: "grab" }}
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
        onPointerUp={endDrag}
        // An interruption (a scroll, an app switch) ends the drag as surely as a
        // release does, and would otherwise leave the view stuck to the mouse.
        onPointerCancel={endDrag}
        onLostPointerCapture={endDrag}
        onPointerLeave={() => {
          pointer.current = null;
          setHover("");
        }}
      />
      <Controls settings={settings} onSettings={onSettings} backend={model.backend} />
      <Stats settings={settings} summary={summary} hover={hover} />
    </div>
  );
}

function Header({
  settings,
  follow,
  onSettings,
  onFollow,
  onFit,
  onClose,
}: {
  settings: HexSettings;
  follow: boolean;
  onSettings: (next: HexSettings) => void;
  onFollow: () => void;
  onFit: () => void;
  onClose: () => void;
}): React.JSX.Element {
  return (
    <div className="flex items-center gap-2 border-b border-edge px-3 py-1.5">
      <span className="text-[11px] font-semibold uppercase tracking-wide text-ink-dim">
        Hex view
      </span>
      <div className="ml-2 flex overflow-hidden rounded border border-edge">
        {(["uniform", "adaptive"] as const).map((mode) => (
          <button
            key={mode}
            type="button"
            onClick={() => onSettings({ ...settings, mode })}
            className={`px-2 py-0.5 capitalize transition-colors ${
              settings.mode === mode ? "bg-[#e0a458] text-black" : "text-ink-dim hover:bg-surface-2"
            }`}
          >
            {mode}
          </button>
        ))}
      </div>
      <button
        type="button"
        onClick={onFollow}
        title="Follow the camera"
        className={`ml-auto rounded p-1 transition-colors hover:bg-surface-2 ${follow ? "text-[#e0a458]" : "text-ink-dim"}`}
      >
        <Crosshair size={13} strokeWidth={1.75} />
      </button>
      <button
        type="button"
        onClick={onFit}
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
  );
}

function Controls({
  settings,
  onSettings,
  backend,
}: {
  settings: HexSettings;
  onSettings: (next: HexSettings) => void;
  /** The adaptive partition is the backend index's, so its sizes are fixed. */
  backend: boolean;
}): React.JSX.Element {
  const set = (patch: Partial<HexSettings>) => onSettings({ ...settings, ...patch });
  return (
    <div className="grid gap-y-1 border-t border-edge px-3 py-2">
      {settings.mode === "uniform" ? (
        <>
          <Slider
            label="Cell size"
            value={settings.size}
            min={100}
            max={1000}
            step={50}
            unit=" m"
            onChange={(size) => set({ size })}
          />
          <Slider
            label="Rings"
            value={settings.radius}
            min={1}
            max={8}
            step={1}
            onChange={(radius) => set({ radius })}
          />
        </>
      ) : (
        <>
          {backend ? (
            <p className="text-[11px] text-ink-dim">
              The partition is the backend index&apos;s: finest cell and split threshold were
              fixed when it was built. The budget below is what the streamer holds.
            </p>
          ) : (
            <>
            <Slider
              label="Finest cell"
              value={settings.fine}
              min={50}
              max={400}
              step={25}
              unit=" m"
              onChange={(fine) => set({ fine })}
            />
            <Slider
              label="Split above"
              value={settings.threshold}
              min={250}
              max={8000}
              step={250}
              onChange={(threshold) => set({ threshold })}
            />
            </>
          )}
          <Slider
            label="Budget"
            value={settings.budget}
            min={10000}
            max={200000}
            step={5000}
            onChange={(budget) => set({ budget })}
          />
        </>
      )}
    </div>
  );
}

function Stats({
  settings,
  summary,
  hover,
}: {
  settings: HexSettings;
  summary: Summary | null;
  hover: string;
}): React.JSX.Element {
  const total = summary?.placements ?? 0;
  const inSet = summary?.setPlacements ?? 0;
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 border-t border-edge px-3 py-2 tabular-nums">
      <dt className="text-ink-dim">World</dt>
      <dd>
        {(summary?.cells ?? 0).toLocaleString()} cells · {total.toLocaleString()} placements · max{" "}
        {(summary?.maxCount ?? 0).toLocaleString()} in one cell
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

import { useCallback, useEffect, useState } from "react";
import { Boxes, FolderTree, Globe2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { formatHash } from "./lib/format";
import { DebugPanel } from "./components/DebugPanel";
import { Explorer } from "./components/Explorer";
import { Onboarding } from "./components/Onboarding";
import { World } from "./components/World";

/** Mirrors the `AppInfo` struct returned by the `app_info` Tauri command. */
type AppInfo = {
  name: string;
  version: string;
  core_check: number;
};

/** The panels the app will grow into. Only the shell exists today. */
const MODES = [
  { id: "explorer", label: "Explorer", icon: FolderTree, phase: 2 },
  { id: "world", label: "World", icon: Globe2, phase: 3 },
  { id: "project", label: "Project", icon: Boxes, phase: 3 },
] as const;

type ModeId = (typeof MODES)[number]["id"];

function App(): React.JSX.Element {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [mode, setMode] = useState<ModeId>("explorer");
  const [debug, setDebug] = useState(false);
  // Counts game folders opened, so the precompile prompt can ask once per open.
  const [gameEpoch, setGameEpoch] = useState(0);
  const onGameOpen = useCallback(() => setGameEpoch((n) => n + 1), []);

  useEffect(() => {
    invoke<AppInfo>("app_info").then(setInfo).catch(console.error);
  }, []);

  // F3 toggles diagnostics. Bound at the window so it works whichever mode is
  // showing and whatever has focus — except a text field, where F3 is not a
  // shortcut anyone means.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== "F3") return;
      const target = event.target as HTMLElement | null;
      if (target?.isContentEditable) return;
      if (target && /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)) return;
      event.preventDefault();
      setDebug((open) => !open);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const active = MODES.find((m) => m.id === mode)!;

  return (
    <div className="flex h-full flex-col bg-surface-0 text-ink">
      <header className="flex h-11 shrink-0 items-center gap-3 border-b border-edge bg-surface-1 px-4">
        <span className="text-sm font-semibold tracking-tight">AstraMapper</span>
        <span className="text-xs text-ink-dim">
          {info ? `v${info.version}` : "starting"}
        </span>
        <button
          type="button"
          onClick={() => setDebug((open) => !open)}
          className={`ml-auto rounded px-2 py-1 text-xs transition-colors ${
            debug ? "bg-surface-2 text-ink" : "text-ink-dim hover:bg-surface-2/60 hover:text-ink"
          }`}
        >
          Diagnostics <kbd className="font-mono">F3</kbd>
        </button>
      </header>

      <div className="flex min-h-0 flex-1">
        <nav className="flex w-48 shrink-0 flex-col gap-1 border-r border-edge bg-surface-1 p-2">
          {MODES.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              onClick={() => setMode(id)}
              className={`flex items-center gap-2 rounded px-3 py-2 text-left text-sm transition-colors ${
                id === mode
                  ? "bg-surface-2 text-ink"
                  : "text-ink-dim hover:bg-surface-2/60 hover:text-ink"
              }`}
            >
              <Icon size={16} strokeWidth={1.75} />
              {label}
            </button>
          ))}
        </nav>

        <main className="relative flex min-w-0 flex-1 flex-col">
          {debug && <DebugPanel onClose={() => setDebug(false)} />}
          <Onboarding epoch={gameEpoch} />
          {mode === "explorer" ? (
            <Explorer onGameOpen={onGameOpen} />
          ) : mode === "world" ? (
            <World />
          ) : (
            <div className="flex flex-1 items-center justify-center p-8">
              <div className="text-center">
                <p className="text-sm text-ink-dim">
                  {active.label} arrives in phase {active.phase}.
                </p>
                {info && (
                  <p className="mt-3 font-mono text-xs text-ink-dim">
                    cw-core linked, jenk(&quot;astramapper&quot;) ={" "}
                    {formatHash(info.core_check)}
                  </p>
                )}
              </div>
            </div>
          )}
        </main>
      </div>
    </div>
  );
}

export default App;

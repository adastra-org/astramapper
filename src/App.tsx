import { useEffect, useState } from "react";
import { Boxes, FolderTree, Globe2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { formatHash } from "./lib/format";

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

  useEffect(() => {
    invoke<AppInfo>("app_info").then(setInfo).catch(console.error);
  }, []);

  const active = MODES.find((m) => m.id === mode)!;

  return (
    <div className="flex h-full flex-col bg-surface-0 text-ink">
      <header className="flex h-11 shrink-0 items-center gap-3 border-b border-edge bg-surface-1 px-4">
        <span className="text-sm font-semibold tracking-tight">AstraMapper</span>
        <span className="text-xs text-ink-dim">
          {info ? `v${info.version}` : "starting"}
        </span>
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

        <main className="flex min-w-0 flex-1 items-center justify-center p-8">
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
        </main>
      </div>
    </div>
  );
}

export default App;

/**
 * First-open prompt: precompile the compiled-model cache now, or let models
 * compile as they are first seen (#7).
 *
 * Shown once per install (a game update asks again) and only while the cache is
 * cold. The recommendation is `planBake` in `src/lib/bakePlan.ts`; the bake is
 * the one the Asset cache panel already drives. This file is wiring and markup.
 */
import { Loader2, X, Zap } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import {
  bakeCancel,
  bakeFinished,
  bakeLabel,
  bakeProgressRatio,
  bakeStart,
  cacheStats,
  type BakeProgress,
} from "../lib/bake";
import { planBake, shouldOfferBake, type Plan, type PlanChoice } from "../lib/bakePlan";
import { getBakeOnboarding, setBakeOnboarding } from "../lib/settings";
import { hardwareInfo, worldStatus } from "../lib/world";

type Offer = { readonly stamp: string; readonly plan: Plan };

const CHOICES: { id: PlanChoice; title: string; detail: string }[] = [
  { id: "all", title: "Everything", detail: "The base game and every DLC pack." },
  { id: "base", title: "Base game only", detail: "Most-placed models first; DLC compiles on demand." },
  { id: "skip", title: "Not now", detail: "Models compile the first time they are seen." },
];

/** `epoch` changes each time a game folder is opened. */
export function Onboarding({ epoch }: { epoch: number }): React.JSX.Element | null {
  const [offer, setOffer] = useState<Offer | null>(null);
  const [choice, setChoice] = useState<PlanChoice>("skip");
  const [progress, setProgress] = useState<BakeProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (epoch === 0) return;
    let cancelled = false;
    void (async () => {
      try {
        const status = await worldStatus();
        if (!status.gameOpen || !status.installStamp) return;
        const [answered, stats, hardware] = await Promise.all([
          getBakeOnboarding(status.installStamp),
          cacheStats(),
          hardwareInfo(),
        ]);
        if (cancelled || !shouldOfferBake(answered !== undefined, stats?.records ?? null)) return;
        const plan = planBake({
          hardware,
          cacheBudgetBytes: stats?.budgetBytes ?? 0,
          cacheUsedBytes: stats?.bytes ?? 0,
          groups: status.dlcGroups,
        });
        setChoice(plan.recommended);
        setProgress(null);
        setError(null);
        setOffer({ stamp: status.installStamp, plan });
      } catch {
        // No prompt is a fine outcome: the bake is still in the Asset cache panel.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [epoch]);

  const running = progress !== null && !bakeFinished(progress);

  const close = useCallback(
    (answer: "baked" | "skipped") => {
      if (offer) void setBakeOnboarding(offer.stamp, answer).catch(() => undefined);
      setOffer(null);
    },
    [offer],
  );

  const start = useCallback(async () => {
    if (!offer) return;
    if (choice === "skip") return close("skipped");
    setError(null);
    try {
      setProgress(await bakeStart(offer.plan.groupsFor[choice], setProgress));
      void setBakeOnboarding(offer.stamp, "baked").catch(() => undefined);
    } catch (cause) {
      setError(String(cause));
      setProgress(null);
    }
  }, [offer, choice, close]);

  if (!offer) return null;
  const { plan } = offer;

  return (
    <div className="absolute inset-0 z-30 flex items-center justify-center bg-black/50 p-4">
      <div
        role="dialog"
        aria-label="Precompile models"
        className="w-[30rem] max-w-full rounded-lg border border-edge bg-surface-1 shadow-xl"
      >
        <header className="flex items-center gap-2 border-b border-edge px-4 py-3">
          <Zap size={16} className="text-ink-dim" />
          <h2 className="text-sm font-medium">Precompile models?</h2>
          {!running && (
            <button
              type="button"
              onClick={() => close(progress ? "baked" : "skipped")}
              title="Close"
              className="ml-auto rounded p-1 text-ink-dim hover:bg-surface-2 hover:text-ink"
            >
              <X size={16} />
            </button>
          )}
        </header>

        <div className="space-y-3 px-4 py-3 text-xs">
          <p className="text-ink-dim">
            A model is slow to compile once and instant afterwards. Doing it now, in the
            background, means the world streams without that wait later.
          </p>

          {progress === null ? (
            <>
              <ul className="list-disc space-y-0.5 pl-4 text-[11px] text-ink-dim">
                {plan.reasons.map((reason) => (
                  <li key={reason}>{reason}</li>
                ))}
              </ul>
              <div className="space-y-1">
                {CHOICES.map(({ id, title, detail }) => (
                  <label
                    key={id}
                    className="flex cursor-pointer items-start gap-2 rounded px-2 py-1.5 hover:bg-surface-2/60"
                  >
                    <input
                      type="radio"
                      name="bake-choice"
                      checked={choice === id}
                      onChange={() => setChoice(id)}
                      className="mt-0.5 accent-[#e0a458]"
                    />
                    <span>
                      <span className="text-ink">{title}</span>
                      {plan.recommended === id && (
                        <span className="ml-2 text-[11px] text-accent">recommended</span>
                      )}
                      <span className="block text-[11px] text-ink-dim">{detail}</span>
                    </span>
                  </label>
                ))}
              </div>
            </>
          ) : (
            <div>
              <div className="h-1 overflow-hidden rounded bg-surface-2">
                <div
                  className="h-full bg-accent transition-[width] duration-200"
                  style={{ width: `${bakeProgressRatio(progress) * 100}%` }}
                />
              </div>
              <p className="pt-1 text-[11px] text-ink-dim">{bakeLabel(progress)}</p>
            </div>
          )}

          {error && <p className="text-[11px] text-red-400">{error}</p>}
        </div>

        <footer className="flex justify-end gap-2 border-t border-edge px-4 py-3">
          {running ? (
            <button
              type="button"
              onClick={() => void bakeCancel()}
              className="flex items-center gap-1.5 rounded bg-surface-2 px-3 py-1.5 text-xs text-ink hover:bg-surface-2/70"
            >
              <Loader2 size={12} className="animate-spin" />
              Stop
            </button>
          ) : progress !== null ? (
            <button
              type="button"
              onClick={() => close("baked")}
              className="rounded bg-surface-2 px-3 py-1.5 text-xs text-ink hover:bg-surface-2/70"
            >
              Done
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void start()}
              className="rounded bg-surface-2 px-3 py-1.5 text-xs text-ink hover:bg-surface-2/70"
            >
              {choice === "skip" ? "Skip" : "Start"}
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}

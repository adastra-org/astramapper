import { memo } from "react";
import { X } from "lucide-react";

type PlacedEntry = {
  readonly key: string;
  readonly name: string;
  readonly placements: { readonly length: number };
};

type Props = {
  entries: readonly PlacedEntry[];
  selectedKey: string | null;
  /** False for the demo plaza, whose one map cannot be removed. */
  removable: boolean;
  onDeselect: () => void;
  onRemove: (key: string) => void;
};

/**
 * The sidebar's list of placed maps.
 *
 * Its own memoized component because `World` re-renders every time a model
 * arrives (the geometry store's version is a React dependency), and with the
 * base game placed this list is 4,601 rows. Inline, every arrival re-rendered
 * all of them: one ~350 ms main-thread task per batch of models, the freeze F3
 * showed as `long task` while `tick js` stayed at ~16 ms. The props here only
 * change when the placed maps or the selection do.
 */
export const PlacedList = memo(function PlacedList({
  entries,
  selectedKey,
  removable,
  onDeselect,
  onRemove,
}: Props): React.JSX.Element {
  return (
    <>
      {entries.map((entry) => (
        <div
          key={entry.key}
          className="group flex items-center gap-2 px-4 py-1 text-xs hover:bg-surface-2/60"
        >
          <button
            type="button"
            onClick={onDeselect}
            className={`min-w-0 flex-1 truncate text-left ${
              selectedKey === entry.key ? "text-accent" : "text-ink"
            }`}
          >
            {entry.name || entry.key}
          </button>
          <span className="shrink-0 text-ink-dim">{entry.placements.length.toLocaleString()}</span>
          {removable && (
            <button
              type="button"
              onClick={() => onRemove(entry.key)}
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
    </>
  );
});

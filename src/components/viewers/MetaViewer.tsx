import { useEffect, useMemo, useState } from "react";
import { Braces, ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { readMeta, type MetaNode, type MetaPreview } from "../../lib/viewer";

/** How many search hits to list before saying there are more. */
const MAX_MATCHES = 500;

/**
 * Show what a `.ymt` holds: the tree of named values the file describes.
 *
 * All three metadata encodings land here. RSC, PSO and RBF store the same kind
 * of thing in three different ways and the reader resolves each against the
 * schema the file carries, so what is on screen is the file's own names and
 * types, not a guess.
 *
 * Branches render only while open. A scenario region names tens of thousands of
 * points and rendering that eagerly would stall the window for no one's benefit.
 */
export function MetaViewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<MetaPreview | null>(null);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setFilter("");
    readMeta()
      .then((loaded) => {
        if (cancelled) return;
        setPreview(loaded);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  const matches = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    if (preview === null || needle === "") return null;
    return search(preview.tree, needle);
  }, [preview, filter]);

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
        <span className="flex items-center gap-1.5">
          <Braces size={13} strokeWidth={1.75} />
          {preview.format}
        </span>
        <span className="truncate text-ink">{preview.rootName}</span>
        <span>{preview.nodeCount.toLocaleString()} nodes</span>
        <input
          type="search"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          placeholder="Filter by name or value"
          className="ml-auto w-56 rounded border border-edge bg-surface-1 px-2 py-0.5 text-xs text-ink placeholder:text-ink-dim focus:outline-none focus:ring-1 focus:ring-edge"
        />
      </div>

      <div className="min-h-0 flex-1 overflow-auto py-1 font-mono text-xs">
        {matches === null ? (
          <MetaRow node={preview.tree} depth={0} open />
        ) : (
          <MetaMatches matches={matches} />
        )}
      </div>
    </div>
  );
}

/** One node, and its children once it is open. */
function MetaRow({
  node,
  depth,
  open: initiallyOpen = false,
}: {
  node: MetaNode;
  depth: number;
  open?: boolean;
}): React.JSX.Element {
  const [open, setOpen] = useState(initiallyOpen);
  const indent = { paddingLeft: `${depth * 12 + 16}px` };
  const label = (
    <>
      <span className={node.error ? "text-red-400" : "text-ink"}>{node.name}</span>
      {node.key !== null && <span className="text-amber-400">[{node.key}]</span>}
      {node.typeName !== null && node.typeName !== node.name && (
        <span className="text-ink-dim">{node.typeName}</span>
      )}
    </>
  );

  if (node.children.length === 0) {
    return (
      <div className="flex gap-2 py-px pr-4" style={indent}>
        {label}
        <span
          className={`min-w-0 flex-1 truncate ${node.error ? "text-red-400" : "text-ink-dim"}`}
        >
          {node.value}
        </span>
      </div>
    );
  }

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen((current) => !current)}
        className="flex w-full items-center gap-1 py-px pr-4 text-left hover:bg-surface-1"
        style={indent}
      >
        {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        <span className="flex gap-2">{label}</span>
        <span className="ml-auto shrink-0 text-ink-dim">{node.children.length}</span>
      </button>
      {open &&
        node.children.map((child, index) => (
          // Position is the identity here: the elements of an array are all
          // named `Item`, and the list is never reordered or spliced.
          // oxlint-disable-next-line react/no-array-index-key
          <MetaRow key={`${child.name}-${index}`} node={child} depth={depth + 1} />
        ))}
    </>
  );
}

/** The hits for a filter, as full paths so a name out of context still reads. */
function MetaMatches({ matches }: { matches: Match[] }): React.JSX.Element {
  if (matches.length === 0) {
    return <p className="px-4 py-2 text-ink-dim">Nothing matches.</p>;
  }
  return (
    <>
      {matches.slice(0, MAX_MATCHES).map((match, index) => (
        // Two hits can share a path and a value, so position is the identity.
        // oxlint-disable-next-line react/no-array-index-key
        <div key={`${match.path}-${index}`} className="flex gap-2 px-4 py-px">
          <span className="shrink-0 text-ink-dim">{match.path}</span>
          <span className="min-w-0 flex-1 truncate text-ink">{match.value}</span>
        </div>
      ))}
      {matches.length > MAX_MATCHES && (
        <p className="px-4 py-2 text-ink-dim">
          and {(matches.length - MAX_MATCHES).toLocaleString()} more
        </p>
      )}
    </>
  );
}

type Match = { readonly path: string; readonly value: string };

/** Every node whose name, key or value contains the needle, with its path. */
function search(root: MetaNode, needle: string): Match[] {
  const found: Match[] = [];
  const walk = (node: MetaNode, path: string): void => {
    const here = path === "" ? node.name : `${path}/${node.name}`;
    const hit =
      node.name.toLowerCase().includes(needle) ||
      (node.key?.toLowerCase().includes(needle) ?? false) ||
      (node.value?.toLowerCase().includes(needle) ?? false);
    if (hit) {
      found.push({ path: node.key === null ? here : `${here}[${node.key}]`, value: node.value ?? "" });
    }
    for (const child of node.children) walk(child, here);
  };
  walk(root, "");
  return found;
}

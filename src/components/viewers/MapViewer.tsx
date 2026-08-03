import { useEffect, useState } from "react";
import {
  Building2,
  ChevronDown,
  ChevronRight,
  Layers,
  Loader2,
  MapPin,
} from "lucide-react";
import { formatHash } from "../../lib/format";
import {
  readMap,
  type MapEntity,
  type MapPreview,
  type MapExtension,
  type YmapPreview,
  type YtypArchetype,
  type YtypPreview,
} from "../../lib/viewer";

/** How many search hits to list before saying there are more. */
const MAX_MATCHES = 500;

/**
 * Show what a `.ymap` or `.ytyp` *means*, rather than the generic XML tree.
 *
 * A `.ymap` is a placement dictionary: every placed entity joined into the
 * parent/child hierarchy `YmapFile.EnsureEntities` builds, plus the car
 * generators, occluders, LOD lights and grass batches that ride along. A
 * `.ytyp` is an archetype dictionary: the records that tell the engine what a
 * model or an interior is, MLO interiors broken down into their rooms, portals
 * and entity sets.
 *
 * Branches render only while open — a city map names tens of thousands of
 * entities, and rendering all of them eagerly would stall the window. The
 * generic tree is still one tab over, for the byte-for-byte truth.
 */
export function MapViewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<MapPreview | null>(null);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setFilter("");
    readMap()
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

  return preview.kind === "ymap" ? (
    <YmapView preview={preview.ymap} filter={filter} onFilter={setFilter} />
  ) : (
    <YtypView preview={preview.ytyp} filter={filter} onFilter={setFilter} />
  );
}

function YmapView({
  preview,
  filter,
  onFilter,
}: {
  preview: YmapPreview;
  filter: string;
  onFilter: (value: string) => void;
}): React.JSX.Element {
  const needle = filter.trim().toLowerCase();
  const matches = needle === "" ? null : searchEntities(preview.roots, needle);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex h-8 shrink-0 items-center gap-3 border-b border-edge px-4 text-xs text-ink-dim">
        <span className="flex items-center gap-1.5">
          <MapPin size={13} strokeWidth={1.75} />
          <span className="text-ink">{preview.name || formatHash(preview.nameHash)}</span>
        </span>
        {preview.parent !== "" && <span className="truncate">parent: {preview.parent}</span>}
        <span>
          {preview.entityCount.toLocaleString()}{" "}
          {preview.entityCount === 1 ? "entity" : "entities"}
        </span>
        {preview.mloEntities > 0 && <span>{preview.mloEntities} MLO</span>}
        <input
          type="search"
          value={filter}
          onChange={(event) => onFilter(event.target.value)}
          placeholder="Filter by archetype"
          className="ml-auto w-52 rounded border border-edge bg-surface-1 px-2 py-0.5 text-xs text-ink placeholder:text-ink-dim focus:outline-none focus:ring-1 focus:ring-edge"
        />
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {matches === null ? (
          preview.roots.map((entity) => <EntityRow key={entity.index} entity={entity} depth={0} />)
        ) : (
          <EntityMatches matches={matches} />
        )}
        {matches === null && preview.roots.length === 0 && (
          <p className="px-4 py-3 text-xs text-ink-dim">This map places no entities.</p>
        )}
      </div>

      <SummaryLine text={`${ymapTotals(preview)} · the tree above is what this map places.`} />
    </div>
  );
}

function YtypView({
  preview,
  filter,
  onFilter,
}: {
  preview: YtypPreview;
  filter: string;
  onFilter: (value: string) => void;
}): React.JSX.Element {
  const needle = filter.trim().toLowerCase();
  const archetypes =
    needle === ""
      ? preview.archetypes
      : preview.archetypes.filter(
          (archetype) =>
            archetype.name.toLowerCase().includes(needle) ||
            archetype.assetName.toLowerCase().includes(needle) ||
            archetype.kind.includes(needle),
        );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex h-8 shrink-0 items-center gap-3 border-b border-edge px-4 text-xs text-ink-dim">
        <span className="flex items-center gap-1.5">
          <Layers size={13} strokeWidth={1.75} />
          <span className="text-ink">{preview.name || formatHash(preview.nameHash)}</span>
        </span>
        <span>
          {preview.archetypes.length.toLocaleString()}{" "}
          {preview.archetypes.length === 1 ? "archetype" : "archetypes"}
        </span>
        {preview.extensions.length > 0 && <span>{preview.extensions.length} extensions</span>}
        {preview.compositeEntityTypes > 0 && (
          <span>{preview.compositeEntityTypes} composite types</span>
        )}
        <input
          type="search"
          value={filter}
          onChange={(event) => onFilter(event.target.value)}
          placeholder="Filter by name or kind"
          className="ml-auto w-52 rounded border border-edge bg-surface-1 px-2 py-0.5 text-xs text-ink placeholder:text-ink-dim focus:outline-none focus:ring-1 focus:ring-edge"
        />
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {archetypes.map((archetype) => (
          <ArchetypeRow key={archetype.index} archetype={archetype} />
        ))}
        {archetypes.length === 0 && (
          <p className="px-4 py-3 text-xs text-ink-dim">
            {preview.archetypes.length === 0
              ? "This dictionary holds no archetypes."
              : "Nothing matches."}
          </p>
        )}
      </div>

      <SummaryLine text="An archetype is what the engine loads when it places a model or an interior. MLO archetypes break down into their rooms, portals and entity sets." />
    </div>
  );
}

/** The counts a map's side structures add up to, as one line. */
function ymapTotals(preview: YmapPreview): string {
  const parts: string[] = [`${preview.entityCount} entities`];
  if (preview.mloEntities > 0) parts.push(`${preview.mloEntities} MLO`);
  if (preview.carGenerators > 0) parts.push(`${preview.carGenerators} car gens`);
  if (preview.timeCycleModifiers > 0)
    parts.push(`${preview.timeCycleModifiers} timecycles`);
  if (preview.boxOccluders + preview.occludeModels > 0)
    parts.push(`${preview.boxOccluders + preview.occludeModels} occluders`);
  if (preview.lodLights > 0) parts.push(`${preview.lodLights} LOD lights`);
  if (preview.distantLodLights > 0)
    parts.push(`${preview.distantLodLights} distant lights`);
  if (preview.grassBatches > 0)
    parts.push(`${preview.grassBatches} grass batches (${preview.grassInstances} instances)`);
  return parts.join(" · ");
}

function SummaryLine({ text }: { text: string }): React.JSX.Element {
  return (
    <div className="shrink-0 border-t border-edge px-4 py-2 text-xs text-ink-dim">{text}</div>
  );
}

/** One entity row, its subtree and its detail fields once open. */
function EntityRow({
  entity,
  depth,
}: {
  entity: MapEntity;
  depth: number;
}): React.JSX.Element {
  const [open, setOpen] = useState(false);
  const hasChildren = entity.children.length > 0;
  const indent = { paddingLeft: `${depth * 16 + 12}px` };

  return (
    <div>
      <button
        type="button"
        onClick={() => setOpen((current) => !current)}
        className="flex w-full items-center gap-2 px-4 py-px text-left text-xs hover:bg-surface-1"
        style={indent}
      >
        {hasChildren ? (
          open ? (
            <ChevronDown size={11} className="shrink-0 text-ink-dim" />
          ) : (
            <ChevronRight size={11} className="shrink-0 text-ink-dim" />
          )
        ) : (
          <span className="w-[11px] shrink-0" />
        )}
        <span className="min-w-0 flex-1 truncate text-ink">
          {entity.name || formatHash(entity.nameHash)}
        </span>
        {entity.isMlo && (
          <span className="shrink-0 rounded bg-surface-2 px-1 py-px text-[10px] text-ink-dim">
            MLO
          </span>
        )}
        {entity.extensions.length > 0 && (
          <span className="shrink-0 rounded bg-surface-2 px-1 py-px text-[10px] text-ink-dim">
            {entity.extensions.length} ext
          </span>
        )}
        {hasChildren && (
          <span className="shrink-0 text-ink-dim">{entity.children.length}</span>
        )}
        <span className="w-24 shrink-0 truncate text-right font-mono text-ink-dim">
          {v3(entity.position)}
        </span>
        <span className="w-16 shrink-0 text-right text-ink-dim">{entity.lodLevel}</span>
      </button>

      {open && (
        <div className="ml-6 mr-4 border-l border-edge py-1 pl-4">
          <Detail label="archetype hash" value={formatHash(entity.nameHash)} />
          <Detail label="guid" value={formatHash(entity.guid)} />
          <Detail label="parent" value={entity.parentIndex === -1 ? "—" : entity.parentIndex} />
          <Detail label="position" value={v3(entity.position)} />
          <Detail label="rotation" value={v4(entity.rotation)} />
          <Detail label="scale" value={`${entity.scaleXy.toFixed(2)}, ${entity.scaleZ.toFixed(2)}`} />
          <Detail label="lod dist" value={fmt(entity.lodDist)} />
          <Detail label="child lod dist" value={fmt(entity.childLodDist)} />
          <Detail label="priority" value={entity.priorityLevel} />
          <Detail label="flags" value={formatHash(entity.flags)} />
          {entity.defaultEntitySets.length > 0 && (
            <Detail label="entity sets" value={entity.defaultEntitySets.join(", ")} />
          )}
          <Extensions extensions={entity.extensions} />
          {entity.children.map((child) => (
            <EntityRow key={child.index} entity={child} depth={0} />
          ))}
        </div>
      )}
    </div>
  );
}

function ArchetypeRow({ archetype }: { archetype: YtypArchetype }): React.JSX.Element {
  const [open, setOpen] = useState(false);
  const mlo = archetype.mlo;

  return (
    <div>
      <button
        type="button"
        onClick={() => setOpen((current) => !current)}
        className="flex w-full items-center gap-2 px-4 py-px text-left text-xs hover:bg-surface-1"
      >
        {open ? (
          <ChevronDown size={11} className="shrink-0 text-ink-dim" />
        ) : (
          <ChevronRight size={11} className="shrink-0 text-ink-dim" />
        )}
        <span className="min-w-0 flex-1 truncate text-ink">
          {archetype.name || formatHash(archetype.nameHash)}
        </span>
        <span className="shrink-0 text-ink-dim">{archetype.kind}</span>
        <span className="w-32 shrink-0 truncate text-right text-ink-dim">{archetype.assetType}</span>
        <span className="w-24 shrink-0 truncate text-right font-mono text-ink-dim">
          {archetype.assetName || formatHash(archetype.assetHash)}
        </span>
      </button>

      {open && (
        <div className="ml-6 mr-4 border-l border-edge py-1 pl-4">
          <Detail label="name hash" value={formatHash(archetype.nameHash)} />
          <Detail label="asset hash" value={formatHash(archetype.assetHash)} />
          <Detail
            label="dictionaries"
            value={[
              archetype.drawableDictionary,
              archetype.textureDictionary,
              archetype.clipDictionary,
              archetype.physicsDictionary,
            ]
              .filter(Boolean)
              .join(", ")}
          />
          <Detail
            label="bounding box"
            value={`${v3(archetype.bbMin)} → ${v3(archetype.bbMax)}`}
          />
          <Detail label="bounds sphere" value={`${v3(archetype.bsCentre)}, r=${fmt(archetype.bsRadius)}`} />
          <Detail label="lod dist" value={fmt(archetype.lodDist)} />
          <Detail label="hd texture dist" value={fmt(archetype.hdTextureDist)} />
          <Detail label="flags" value={formatHash(archetype.flags)} />
          {archetype.timeFlags !== null && (
            <Detail label="time flags" value={formatHash(archetype.timeFlags)} />
          )}
          <Extensions extensions={archetype.extensions} />

          {mlo !== null && (
            <div className="mt-1">
              <MloSection title="Rooms" count={mlo.rooms.length}>
                {mlo.rooms.map((room) => (
                  <div key={room.index} className="py-px">
                    <span className="text-ink">{room.name || `room ${room.index}`}</span>
                    <span className="ml-2 text-ink-dim">
                      {v3(room.bbMin)} → {v3(room.bbMax)}
                      {room.portalCount > 0 && ` · ${room.portalCount} portals`}
                      {room.attachedObjects > 0 && ` · ${room.attachedObjects} attached`}
                    </span>
                  </div>
                ))}
              </MloSection>
              <MloSection title="Portals" count={mlo.portals.length}>
                {mlo.portals.map((portal) => (
                  <div key={portal.index} className="py-px text-ink-dim">
                    room {portal.roomFrom} → room {portal.roomTo}
                    {portal.corners > 0 && ` · ${portal.corners} corners`}
                  </div>
                ))}
              </MloSection>
              <MloSection title="Entity sets" count={mlo.entitySets.length}>
                {mlo.entitySets.map((set) => (
                  <div key={set.index} className="py-px">
                    <span className="text-ink">{set.name || formatHash(set.nameHash)}</span>
                    <span className="ml-2 text-ink-dim">
                      {set.entities} {set.entities === 1 ? "entity" : "entities"}
                      {set.locations > 0 && ` · ${set.locations} locations`}
                    </span>
                  </div>
                ))}
              </MloSection>
              <MloSection
                title="Entities"
                count={mlo.entities.length}
                collapsible
                initialOpen={false}
              >
                {mlo.entities.map((entity) => (
                  <EntityRow key={entity.index} entity={entity} depth={0} />
                ))}
              </MloSection>
              {mlo.timeCycleModifiers > 0 && (
                <p className="py-px text-ink-dim">
                  {mlo.timeCycleModifiers} timecycle{" "}
                  {mlo.timeCycleModifiers === 1 ? "modifier" : "modifiers"}
                </p>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** A titled section of an MLO interior. */
function MloSection({
  title,
  count,
  collapsible = false,
  initialOpen = true,
  children,
}: {
  title: string;
  count: number;
  collapsible?: boolean;
  initialOpen?: boolean;
  children: React.ReactNode;
}): React.JSX.Element {
  const [open, setOpen] = useState(initialOpen);
  if (count === 0) return <></>;
  const header = (
    <>
      {collapsible &&
        (open ? (
          <ChevronDown size={11} className="text-ink-dim" />
        ) : (
          <ChevronRight size={11} className="text-ink-dim" />
        ))}
      {!collapsible && <Building2 size={11} strokeWidth={1.75} className="text-ink-dim" />}
      {title} · {count}
    </>
  );
  return (
    <div className="py-px">
      {collapsible ? (
        <button
          type="button"
          onClick={() => setOpen((current) => !current)}
          className="flex items-center gap-1.5 text-ink hover:text-ink-dim"
        >
          {header}
        </button>
      ) : (
        <p className="flex items-center gap-1.5 text-ink">{header}</p>
      )}
      {open && <div>{children}</div>}
    </div>
  );
}

function Extensions({ extensions }: { extensions: readonly MapExtension[] }): React.JSX.Element | null {
  if (extensions.length === 0) return null;
  return (
    <Detail
      label="extensions"
      value={extensions
        .map((extension) => extension.name || extension.structure)
        .join(", ")}
    />
  );
}

function Detail({ label, value }: { label: string; value: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex gap-2 py-px">
      <span className="w-32 shrink-0 truncate text-ink-dim">{label}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-ink">{value}</span>
    </div>
  );
}

type Match = { readonly path: string; readonly entity: MapEntity };

/** Every entity whose archetype name or LOD level contains the needle. */
function searchEntities(roots: readonly MapEntity[], needle: string): Match[] {
  const found: Match[] = [];
  const walk = (entity: MapEntity, path: string): void => {
    const here = path === "" ? entity.name : `${path} / ${entity.name}`;
    if (entity.name.toLowerCase().includes(needle) || entity.lodLevel.toLowerCase().includes(needle)) {
      found.push({ path: here, entity });
    }
    for (const child of entity.children) walk(child, here);
  };
  for (const root of roots) walk(root, "");
  return found;
}

function EntityMatches({ matches }: { matches: Match[] }): React.JSX.Element {
  if (matches.length === 0) {
    return <p className="px-4 py-2 text-xs text-ink-dim">Nothing matches.</p>;
  }
  return (
    <>
      {matches.slice(0, MAX_MATCHES).map((match, index) => (
        // Two hits can share a path and a name, so position is the identity.
        // oxlint-disable-next-line react/no-array-index-key
        <div key={`${match.path}-${index}`} className="flex gap-2 px-4 py-px text-xs">
          <span className="min-w-0 flex-1 truncate text-ink">
            {match.entity.name || formatHash(match.entity.nameHash)}
          </span>
          <span className="shrink-0 text-ink-dim">{match.entity.lodLevel}</span>
          <span className="w-24 shrink-0 truncate text-right font-mono text-ink-dim">
            {v3(match.entity.position)}
          </span>
          <span className="shrink-0 truncate text-ink-dim">{match.path}</span>
        </div>
      ))}
      {matches.length > MAX_MATCHES && (
        <p className="px-4 py-2 text-xs text-ink-dim">
          and {(matches.length - MAX_MATCHES).toLocaleString()} more
        </p>
      )}
    </>
  );
}

function v3(value: readonly number[]): string {
  return value.map((n) => n.toFixed(2)).join(", ");
}

function v4(value: readonly number[]): string {
  return value.map((n) => n.toFixed(3)).join(", ");
}

function fmt(value: number): string {
  return Number.isFinite(value) ? value.toFixed(2) : "—";
}

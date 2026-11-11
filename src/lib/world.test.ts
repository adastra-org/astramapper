import { describe, expect, test } from "bun:test";
import { ARCHETYPE_CLASS, columnsFromPlaced, type PlacementColumns } from "./world/columns";
import {
  applyEdit,
  axisRotate,
  boundsOf,
  colorForLod,
  commitHistory,
  demoWorld,
  editOf,
  entityStreamRadius,
  gtaToWorld,
  keyFor,
  parseKey,
  lodBandForDistance,
  nearestCandidates,
  parseProject,
  placementScale,
  popHistory,
  serializeProject,
  STREAM_VIEW_NEAR,
  stillStreamWanted,
  streamingDecision,
  streamPenalty,
  STREAM_PHASE_PENALTY,
  STREAM_VIEW_COS_HALF_ANGLE,
  worldToGta,
  type Placed,
  type RegionEntry,
  type StreamView,
  type TransformEdit,
} from "./world";

/** A map entry with one placed entity per position, for the streamer tests. */
function entry(key: string, positions: readonly (readonly [number, number, number])[]): {
  key: string;
  placements: PlacementColumns;
} {
  return {
    key,
    placements: columnsFromPlaced(positions.map((position, index) => ({
      index,
      name: "x",
      nameHash: 0,
      isMlo: false,
      fromMlo: false,
      position,
      rotation: [0, 0, 0, 1],
      scaleXy: 1,
      scaleZ: 1,
      parentIndex: -1,
      lodLevel: "HD",
      lodDist: 100,
      children: 0,
    }))),
  };
}

/**
 * The `invoke` wrappers need a Tauri host to say anything, so only the pure
 * geometry and colour helpers are covered here; the commands behind the
 * wrappers are exercised by `src-tauri/tests/world_gate.rs`.
 */
describe("placementScale", () => {
  test("maps the ground-plane scale onto the world's X and Z", () => {
    expect(placementScale(2, 5)).toEqual([2, 5, 2]);
  });

  test("keeps a uniform scale uniform", () => {
    expect(placementScale(3, 3)).toEqual([3, 3, 3]);
  });
});

describe("gtaToWorld", () => {
  test("raises the Z-up axis into three.js's Y-up world", () => {
    expect(gtaToWorld([1, 2, 3])).toEqual([1, 3, -2]);
  });

  test("keeps the ground plane flat", () => {
    const world = gtaToWorld([0, 0, 0]);
    // `toBe` treats `-0` and `0` as different, and the sign is irrelevant here.
    expect(world[0] === 0).toBe(true);
    expect(world[1] === 0).toBe(true);
    expect(world[2] === 0).toBe(true);
  });

  test("round-trips with worldToGta", () => {
    const gta: [number, number, number] = [12.5, -3.25, 700];
    expect(worldToGta(gtaToWorld(gta))).toEqual(gta);
  });
});

describe("keyFor", () => {
  test("joins a map and an entity into a stable identity", () => {
    expect(keyFor("m", 0)).toBe("m:0");
    expect(keyFor("m", 12)).toBe("m:12");
  });
});

describe("parseKey", () => {
  test("round-trips every key keyFor produces", () => {
    for (const mapKey of [
      "demo",
      "x64a.rpf\\levels\\gta5\\_citye\\downtown\\dt1_02.ymap",
      // A map placed from disk keeps its absolute path, drive colon and all.
      "E:\\Ad Astra Tools\\maps\\custom.ymap",
      // And a nested archive path has a colon in the middle of it.
      "update.rpf\\x64:dlc\\interior.ymap",
    ]) {
      for (const entityIndex of [0, 7, 1_234_567]) {
        expect(parseKey(keyFor(mapKey, entityIndex))).toEqual({ mapKey, entityIndex });
      }
    }
  });

  test("splits on the last separator, not the first", () => {
    expect(parseKey("E:\\maps\\a.ymap:9")).toEqual({
      mapKey: "E:\\maps\\a.ymap",
      entityIndex: 9,
    });
  });

  test("refuses anything that is not one of its keys", () => {
    expect(parseKey("nocolon")).toBeNull();
    expect(parseKey("m:")).toBeNull();
    expect(parseKey(":12")).toBeNull();
    expect(parseKey("m:abc")).toBeNull();
    expect(parseKey("m:-1")).toBeNull();
    expect(parseKey("m: 12")).toBeNull();
    expect(parseKey("m:1e3")).toBeNull();
    expect(parseKey("m:0x10")).toBeNull();
  });
});

describe("lodBandForDistance", () => {
  test("uses the entity's LOD distance as the HD boundary", () => {
    const lodDist = 100;
    expect(lodBandForDistance(10, lodDist, 4)).toBe(0);
    expect(lodBandForDistance(100, lodDist, 4)).toBe(0);
    expect(lodBandForDistance(120, lodDist, 4)).toBe(1);
    expect(lodBandForDistance(200, lodDist, 4)).toBe(1);
    expect(lodBandForDistance(300, lodDist, 4)).toBe(2);
    expect(lodBandForDistance(1000, lodDist, 4)).toBe(3);
  });

  test("falls back to fixed bands without a LOD distance", () => {
    expect(lodBandForDistance(50, 0, 4)).toBe(0);
    expect(lodBandForDistance(200, 0, 4)).toBe(1);
    expect(lodBandForDistance(500, 0, 4)).toBe(2);
    expect(lodBandForDistance(900, 0, 4)).toBe(2);
    expect(lodBandForDistance(5000, 0, 4)).toBe(3);
  });

  test("clamps to the bundles the model actually has", () => {
    expect(lodBandForDistance(100000, 100, 2)).toBe(1);
    expect(lodBandForDistance(100000, 100, 1)).toBe(0);
  });
});

describe("nearestCandidates", () => {
  const entries = [
    entry("m1", [
      [0, 0, 0],
      [100, 0, 0],
    ]),
    entry("m2", [[30, 0, 0]]),
  ];
  test("returns the nearest unexcluded entities, nearest first", () => {
    const got = nearestCandidates([0, 0, 0], entries, new Set(), 200, 10);
    // Per-map indexes: m1's second entity (index 1) is farthest.
    expect(got.map((c) => c.entityIndex)).toEqual([0, 0, 1]);
    expect(got[0]).toMatchObject({ mapKey: "m1", entityIndex: 0 });
  });

  test("honours the excluded set", () => {
    const got = nearestCandidates([0, 0, 0], entries, new Set(["m1:0"]), 200, 10);
    expect(got.map((c) => c.entityIndex)).toEqual([0, 1]);
  });

  test("culls by radius and caps by limit", () => {
    const near = nearestCandidates([0, 0, 0], entries, new Set(), 50, 10);
    expect(near.map((c) => c.entityIndex)).toEqual([0, 0]);
    const capped = nearestCandidates([0, 0, 0], entries, new Set(), 200, 1);
    expect(capped).toHaveLength(1);
  });
});

describe("colorForLod", () => {
  test("steps the brightness down as LOD distance grows", () => {
    const hd = colorForLod("HD", false);
    const lod = colorForLod("LOD", false);
    const slod = colorForLod("SLOD1", false);
    expect(hd).not.toBe(lod);
    expect(lod).not.toBe(slod);
  });

  test("gives interior instances their own hue over the LOD colour", () => {
    expect(colorForLod("HD", true)).toBe(colorForLod("LOD", true));
    expect(colorForLod("HD", true)).not.toBe(colorForLod("HD", false));
  });

  test("has a stable colour for unknown LOD labels", () => {
    expect(colorForLod("", false)).toBe(colorForLod("weird", false));
  });
});

describe("boundsOf", () => {
  test("is a single point for one placement", () => {
    const { min, max, center, radius } = boundsOf([{ position: [1, 2, 3] }]);
    expect(min).toEqual([1, 2, 3]);
    expect(max).toEqual([1, 2, 3]);
    expect(center).toEqual([1, 2, 3]);
    expect(radius).toBeGreaterThanOrEqual(1);
  });

  test("is degenerate but finite for nothing", () => {
    const { center, radius } = boundsOf([]);
    expect(center).toEqual([0, 0, 0]);
    expect(radius).toBe(1);
  });

  test("contains every placement", () => {
    const placements = [
      { position: [-10, -20, -30] as const },
      { position: [5, 6, 7] as const },
    ];
    const { min, max, center } = boundsOf(placements);
    expect(min).toEqual([-10, -20, -30]);
    expect(max).toEqual([5, 6, 7]);
    expect(center).toEqual([-2.5, -7, -11.5]);
  });
});

describe("demoWorld", () => {
  test("renders a deterministic, non-empty plaza", () => {
    const a = demoWorld();
    const b = demoWorld();
    expect(a.name).toBe("demo_plaza");
    expect(a.placements.length).toBeGreaterThan(0);
    expect(a.placements.length).toBe(b.placements.length);
    expect(a.placements[0]).toEqual(b.placements[0]);
  });

  test("every placement is finite and standing on the ground", () => {
    const { placements } = demoWorld();
    for (const placement of placements) {
      expect(placement.position.every(Number.isFinite)).toBe(true);
      expect(placement.scaleXy).toBeGreaterThan(0);
      expect(placement.scaleZ).toBeGreaterThan(0);
      expect(placement.position[2] + placement.scaleZ / 2).toBeGreaterThanOrEqual(0);
    }
  });

  test("has an interior marker so the MLO colour shows", () => {
    const { placements } = demoWorld();
    expect(placements.some((p) => p.isMlo)).toBe(true);
  });
});

describe("applyEdit and editOf", () => {
  const placement: Placed = {
    index: 0,
    name: "x",
    nameHash: 0,
    isMlo: false,
    fromMlo: false,
    position: [1, 2, 3],
    rotation: [0, 0, 0, 1],
    scaleXy: 1,
    scaleZ: 1,
    parentIndex: -1,
    lodLevel: "HD",
    lodDist: 100,
    children: 0,
  };

  test("is the identity for no edit", () => {
    expect(applyEdit(placement, undefined)).toBe(placement);
  });

  test("replaces the stored transform when edited", () => {
    const edit: TransformEdit = {
      position: [10, 20, 30],
      rotation: [0, 0, 1, 0],
      scaleXy: 2,
      scaleZ: 3,
    };
    const edited = applyEdit(placement, edit);
    expect(edited.position).toEqual([10, 20, 30]);
    expect(edited.rotation).toEqual([0, 0, 1, 0]);
    expect(edited.scaleXy).toBe(2);
    expect(edited.scaleZ).toBe(3);
    expect(edited.name).toBe("x");
    expect(placement.position).toEqual([1, 2, 3]);
  });

  test("round-trips through editOf", () => {
    const edit = editOf(placement);
    expect(applyEdit(placement, edit)).toEqual(placement);
  });
});

describe("axisRotate", () => {
  test("rotates the identity around Z", () => {
    const q = axisRotate([0, 0, 0, 1], [0, 0, 1], Math.PI / 2);
    expect(q[0]).toBeCloseTo(0, 5);
    expect(q[1]).toBeCloseTo(0, 5);
    expect(q[2]).toBeCloseTo(Math.SQRT1_2, 5);
    expect(q[3]).toBeCloseTo(Math.SQRT1_2, 5);
  });

  test("is the identity for a zero angle", () => {
    const q = axisRotate([0.1, 0.2, 0.3, 0.9], [1, 0, 0], 0);
    expect(q).toEqual([0.1, 0.2, 0.3, 0.9]);
  });

  test("stays a unit quaternion", () => {
    const start: [number, number, number, number] = [0, 0, Math.SQRT1_2, Math.SQRT1_2];
    const q = axisRotate(start, [0, 1, 0], 0.7);
    const norm = Math.hypot(q[0], q[1], q[2], q[3]);
    expect(norm).toBeCloseTo(1, 5);
  });
});

describe("project files", () => {
  const edit: TransformEdit = {
    position: [10, 0, 0],
    rotation: [0, 0, Math.SQRT1_2, Math.SQRT1_2],
    scaleXy: 2,
    scaleZ: 3,
  };

  test("round-trips maps and edits", () => {
    const edits = new Map<string, TransformEdit>([["m:0", edit]]);
    const text = serializeProject(["x64a.rpf\\map.ymap"], edits);
    const parsed = parseProject(text);
    expect(parsed.maps).toEqual(["x64a.rpf\\map.ymap"]);
    expect(parsed.edits).toEqual([{ key: "m:0", edit }]);
  });

  test("serializes an empty project", () => {
    const parsed = parseProject(serializeProject([], new Map()));
    expect(parsed.maps).toEqual([]);
    expect(parsed.edits).toEqual([]);
  });

  test("rejects a file that is not an AstraMapper project", () => {
    expect(() => parseProject(JSON.stringify({ format: "other", version: 1 }))).toThrow(
      "not an AstraMapper project",
    );
  });

  test("rejects an unsupported version", () => {
    expect(() =>
      parseProject(JSON.stringify({ format: "astramapper", version: 99, maps: [] })),
    ).toThrow("unsupported project version");
  });

  test("drops malformed edits rather than failing the file", () => {
    const text = JSON.stringify({
      format: "astramapper",
      version: 1,
      maps: ["a"],
      edits: { "m:0": { position: [1, 2], rotation: [0, 0, 0, 1], scaleXy: 1, scaleZ: 1 } },
    });
    const parsed = parseProject(text);
    expect(parsed.maps).toEqual(["a"]);
    expect(parsed.edits).toEqual([]);
  });
});

describe("entityStreamRadius", () => {
  test("grows with the LOD tier", () => {
    const near = entityStreamRadius({ ...entryFixture, lodLevel: "ORPHAN_HD" });
    const hd = entityStreamRadius({ ...entryFixture, lodLevel: "HD" });
    const lod = entityStreamRadius({ ...entryFixture, lodLevel: "LOD" });
    const slod = entityStreamRadius({ ...entryFixture, lodLevel: "SLOD2" });
    expect(near).toBeLessThan(hd);
    expect(hd).toBeLessThan(lod);
    expect(lod).toBeLessThan(slod);
  });

  test("floors a degenerate LOD distance", () => {
    expect(entityStreamRadius({ ...entryFixture, lodDist: 0 })).toBe(50 * 7);
  });
});

describe("streamPenalty", () => {
  const penalty = (name: string, lodLevel = "HD") =>
    streamPenalty({ ...entryFixture, name, lodLevel });

  test("orders terrain, structures, decals, vegetation, then coarse LOD", () => {
    const order = [
      penalty("cs1_01_terrain_a"),
      penalty("prop_building_01"),
      penalty("v_decal_road_02"),
      penalty("prop_tree_oak_01"),
      penalty("prop_building_01", "SLOD2"),
    ];
    expect(order).toEqual([
      STREAM_PHASE_PENALTY.terrain,
      STREAM_PHASE_PENALTY.structure,
      STREAM_PHASE_PENALTY.decal,
      STREAM_PHASE_PENALTY.vegetation,
      STREAM_PHASE_PENALTY.farLod,
    ]);
  });

  describe("archetype class from the data (#9)", () => {
    const classed = (name: string, archetypeClass: number, lodLevel = "HD") =>
      streamPenalty({ ...entryFixture, name, lodLevel, archetypeClass });

    test("a decal or vegetation class decides without any name token", () => {
      expect(classed("ap1_01_c_puds_06", ARCHETYPE_CLASS.decal)).toBe(STREAM_PHASE_PENALTY.decal);
      expect(classed("prop_coral_kelp_01", ARCHETYPE_CLASS.vegetation)).toBe(
        STREAM_PHASE_PENALTY.vegetation,
      );
      // Whatever the LOD tier: the class is about what the thing is.
      expect(classed("ap1_01_c_puds_06", ARCHETYPE_CLASS.decal, "SLOD2")).toBe(
        STREAM_PHASE_PENALTY.decal,
      );
    });

    test("the data wins over a name that says something else", () => {
      expect(classed("prop_tree_card", ARCHETYPE_CLASS.decal)).toBe(STREAM_PHASE_PENALTY.decal);
      expect(classed("v_decal_road_02", ARCHETYPE_CLASS.vegetation)).toBe(
        STREAM_PHASE_PENALTY.vegetation,
      );
    });

    test("no class, or an absent one, falls back to the name tokens", () => {
      expect(classed("prop_tree_oak_01", ARCHETYPE_CLASS.none)).toBe(
        STREAM_PHASE_PENALTY.vegetation,
      );
      expect(penalty("v_decal_road_02")).toBe(STREAM_PHASE_PENALTY.decal);
      expect(classed("prop_building_01", ARCHETYPE_CLASS.none)).toBe(
        STREAM_PHASE_PENALTY.structure,
      );
    });

    test("terrain stays on its name: no archetype data identifies ground", () => {
      expect(classed("cs1_01_terrain_a", ARCHETYPE_CLASS.vegetation)).toBe(
        STREAM_PHASE_PENALTY.terrain,
      );
    });

    test("an unknown class number is ignored rather than trusted", () => {
      expect(classed("prop_building_01", 99)).toBe(STREAM_PHASE_PENALTY.structure);
    });
  });

  test("matches whole name tokens, not substrings", () => {
    // `powerplant` and `terrace` contain plant / ter but are neither.
    expect(penalty("prop_powerplant_01")).toBe(STREAM_PHASE_PENALTY.structure);
    expect(penalty("prop_terrace_01")).toBe(STREAM_PHASE_PENALTY.structure);
  });
});

describe("streamingDecision", () => {
  test("phases are strict tiers: all terrain, then structures, then vegetation, whatever the distance", () => {
    const entries = [
      regionEntryOf("m1", [
        placedAtXy("prop_building_01", 0, 50, 0),
        placedAtXy("prop_tree_oak_01", 1, 10, 0),
        placedAtXy("prop_building_02", 2, 400, 0),
        placedAtXy("terrain_a", 3, 90, 0),
      ]),
    ];
    const { load } = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4);
    const names = load.map((c) => c.entityIndex);
    // terrain first although it is farther than the tree; the tree last
    // although it is nearer than the far building.
    expect(names).toEqual([3, 0, 2, 1]);
  });

  test("a classified decal and vegetation load in their phases though their names say nothing", () => {
    const classed = (name: string, index: number, x: number, archetypeClass: number): Placed => ({
      ...placedAtXy(name, index, x, 0),
      archetypeClass,
    });
    const entries = [
      regionEntryOf("m1", [
        classed("prop_kelp_01", 0, 10, ARCHETYPE_CLASS.vegetation),
        classed("ap1_puddle_a", 1, 20, ARCHETYPE_CLASS.decal),
        placedAtXy("prop_building_01", 2, 80, 0),
      ]),
    ];
    const { load } = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4);
    // structure, then the decal, then the vegetation, though the kelp is nearest.
    expect(load.map((c) => c.entityIndex)).toEqual([2, 1, 0]);
  });

  const region = (
    key: string,
    placements: Placed[],
  ): ReturnType<typeof regionEntryOf> => regionEntryOf(key, placements);

  describe("whole-map terrain layer (#5)", () => {
    test("terrain loads from outside its map's box and radius; structures there do not", () => {
      const entries = [
        region("m1", [
          placedAtXy("terrain_a", 0, 20_000, 0),
          placedAtXy("prop_building_01", 1, 20_000, 10),
        ]),
      ];
      const { load } = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4);
      expect(load.map((c) => c.entityIndex)).toEqual([0]);
    });

    test("terrain from every map comes before any structure, nearest terrain first", () => {
      const entries = [
        region("m1", [placedAtXy("prop_building_01", 0, 10, 0), placedAtXy("terrain_a", 1, 5_000, 0)]),
        region("m2", [placedAtXy("terrain_b", 0, 800, 0)]),
      ];
      const { load } = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4);
      expect(load.map((c) => `${c.mapKey}:${c.entityIndex}`)).toEqual(["m2:0", "m1:1", "m1:0"]);
    });

    test("loaded terrain never unloads, in range or not; a structure there does", () => {
      const entries = [
        region("m1", [placedAtXy("terrain_a", 0, 20_000, 0), placedAtXy("prop_building_01", 1, 20_000, 0)]),
      ];
      const decision = streamingDecision(
        [0, 0, 0],
        entries,
        new Set(["m1:0", "m1:1"]),
        new Set(),
        4,
      );
      expect(decision.unload).toEqual(["m1:1"]);

      const near = streamingDecision(
        [20_000, 0, 0],
        [region("m1", [placedAtXy("terrain_a", 0, 5_000, 0)])],
        new Set(["m1:0"]),
        new Set(),
        4,
      );
      expect(near.unload).toEqual([]);
    });

    test("does not re-queue loaded or pending terrain", () => {
      const entries = [region("m1", [placedAtXy("terrain_a", 0, 20_000, 0), placedAtXy("terrain_b", 1, 20_000, 0)])];
      const decision = streamingDecision(
        [0, 0, 0],
        entries,
        new Set(["m1:0"]),
        new Set(["m1:1"]),
        4,
      );
      expect(decision.load).toEqual([]);
    });

    test("a waiting terrain request stays wanted wherever the camera goes", () => {
      const placement = placedAtXy("terrain_a", 0, 20_000, 0);
      const box = { extentsMin: [0, 0, 0], extentsMax: [100, 100, 100] } as const;
      expect(stillStreamWanted([0, 0, 0], box, placement)).toBe(true);
      expect(stillStreamWanted([0, 0, 0], box, { ...placement, name: "prop_building_01" })).toBe(false);
    });
  });

  test("loads the nearest in-range entities, capped by the batch", () => {
    const entries = [region("m1", [placedAt("m1", 0, 0), placedAt("m1", 0, 1000)])];
    const decision = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 1);
    expect(decision.load).toHaveLength(1);
    expect(decision.load[0]).toMatchObject({ mapKey: "m1", entityIndex: 0 });
    expect(decision.unload).toEqual([]);
  });

  test("unloads entities beyond their radius with hysteresis", () => {
    const entries = [region("m1", [placedAt("m1", 0, 2000)])];
    // 2000 is well beyond HD radius * 1.5 (700 * 1.5 = 1050).
    const decision = streamingDecision([0, 0, 0], entries, new Set(["m1:0"]), new Set(), 4);
    expect(decision.unload).toEqual(["m1:0"]);
  });

  test("drops a map whose streaming box the camera has left", () => {
    const entries = [region("m1", [placedAt("m1", 0, 10)])];
    // Camera far beyond the box and its margin: nothing loads, all unload.
    const decision = streamingDecision([5000, 0, 0], entries, new Set(["m1:0"]), new Set(), 4);
    expect(decision.load).toEqual([]);
    expect(decision.unload).toEqual(["m1:0"]);
  });

  test("never re-queues a loaded or pending entity", () => {
    const entries = [region("m1", [placedAt("m1", 0, 10)])];
    const decision = streamingDecision(
      [0, 0, 0],
      entries,
      new Set(["m1:0"]),
      new Set(["m1:0"]),
      4,
    );
    expect(decision.load).toEqual([]);
    expect(decision.unload).toEqual([]);
  });

  test("orders by distance alone when there is no view", () => {
    // Entity 0 is behind the camera and nearest; with no view it still wins.
    const entries = [
      region("m1", [
        placedAtXy("m1", 0, -100, 0),
        placedAtXy("m1", 1, 400, 0),
        placedAtXy("m1", 2, 200, 0),
      ]),
    ];
    const decision = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 3);
    expect(decision.load.map((c) => c.entityIndex)).toEqual([0, 2, 1]);
  });

  describe("with a view", () => {
    /** Looking down +X in the game's Z-up space. */
    const lookingAlongX: StreamView = {
      forward: [1, 0, 0],
      cosHalfAngle: STREAM_VIEW_COS_HALF_ANGLE,
    };

    test("fills the batch with what is on screen before what is behind", () => {
      // The near one is behind the camera; the far one is straight ahead.
      const entries = [
        region("m1", [
          placedAtXy("m1", 0, -100, 0),
          placedAtXy("m1", 1, 400, 0),
        ]),
      ];
      const decision = streamingDecision(
        [0, 0, 0],
        entries,
        new Set(),
        new Set(),
        2,
        lookingAlongX,
      );

      expect(decision.load.map((c) => c.entityIndex)).toEqual([1, 0]);
      expect(decision.load.map((c) => c.inView)).toEqual([true, false]);
    });

    test("a batch too small to hold both keeps the one on screen", () => {
      const entries = [
        region("m1", [placedAtXy("m1", 0, -100, 0), placedAtXy("m1", 1, 400, 0)]),
      ];
      const decision = streamingDecision(
        [0, 0, 0],
        entries,
        new Set(),
        new Set(),
        1,
        lookingAlongX,
      );

      expect(decision.load).toHaveLength(1);
      expect(decision.load[0]!.entityIndex).toBe(1);
    });

    test("still orders by distance among the entities on screen", () => {
      const entries = [
        region("m1", [
          placedAtXy("m1", 0, 500, 0),
          placedAtXy("m1", 1, 200, 0),
          placedAtXy("m1", 2, 350, 0),
        ]),
      ];
      const decision = streamingDecision(
        [0, 0, 0],
        entries,
        new Set(),
        new Set(),
        3,
        lookingAlongX,
      );

      expect(decision.load.map((c) => c.entityIndex)).toEqual([1, 2, 0]);
      expect(decision.load.every((c) => c.inView)).toBe(true);
    });

    /**
     * A cone has nothing at its apex. Without the near rule the models the
     * camera is standing among score as behind it and load last.
     */
    test("counts what the camera is standing among as on screen", () => {
      const entries = [
        region("m1", [
          placedAtXy("m1", 0, -10, 0), // behind, but a few metres away
          placedAtXy("m1", 1, 600, 0), // dead ahead, far
        ]),
      ];
      const decision = streamingDecision(
        [0, 0, 0],
        entries,
        new Set(),
        new Set(),
        2,
        lookingAlongX,
      );

      expect(decision.load.map((c) => c.inView)).toEqual([true, true]);
      expect(decision.load.map((c) => c.entityIndex)).toEqual([0, 1]);
    });

    /**
     * The point of ordering rather than culling. Something behind the camera
     * must still be a candidate, or turning around finds nothing there and the
     * same geometry streams back in as a burst.
     */
    test("does not cull what is behind the camera out of the candidates", () => {
      const entries = [region("m1", [placedAtXy("m1", 0, -300, 0)])];
      const decision = streamingDecision(
        [0, 0, 0],
        entries,
        new Set(),
        new Set(),
        4,
        lookingAlongX,
      );

      expect(decision.load).toHaveLength(1);
      expect(decision.load[0]!.inView).toBe(false);
    });

    describe("off-view radius", () => {
      // HD at lodDist 100: radius 700, off-view cut 350, unload at 1050.
      test("an entity behind the camera loads only inside half its radius", () => {
        const entries = [
          region("m1", [placedAtXy("m1", 0, -300, 0), placedAtXy("m1", 1, -500, 0)]),
        ];
        const decision = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4, lookingAlongX);
        expect(decision.load.map((c) => c.entityIndex)).toEqual([0]);
      });

      test("the same entity ahead of the camera loads out to its full radius", () => {
        const entries = [region("m1", [placedAtXy("m1", 0, 500, 0), placedAtXy("m1", 1, 650, 0)])];
        const decision = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4, lookingAlongX);
        expect(decision.load.map((c) => c.entityIndex)).toEqual([0, 1]);
      });

      test("without a view the whole sphere is a candidate", () => {
        const entries = [region("m1", [placedAtXy("m1", 0, -500, 0)])];
        const decision = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4);
        expect(decision.load.map((c) => c.entityIndex)).toEqual([0]);
      });

      test("inside the near radius is on screen whatever the angle", () => {
        const entries = [region("m1", [placedAtXy("m1", 0, -STREAM_VIEW_NEAR + 1, 0)])];
        const decision = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4, lookingAlongX);
        expect(decision.load[0]!.inView).toBe(true);
      });

      test("scales with the tier: a coarse LOD placement reaches further behind", () => {
        const far = { ...placedAtXy("m1", 0, -600, 0), lodLevel: "LOD" }; // radius 1400, cut 700
        const entries = [region("m1", [far])];
        const decision = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4, lookingAlongX);
        expect(decision.load.map((c) => c.entityIndex)).toEqual([0]);
      });

      test("terrain behind the camera is still wanted at any distance", () => {
        const entries = [region("m1", [placedAtXy("terrain_a", 0, -5_000, 0)])];
        const decision = streamingDecision([0, 0, 0], entries, new Set(), new Set(), 4, lookingAlongX);
        expect(decision.load.map((c) => c.entityIndex)).toEqual([0]);
      });

      test("a loaded entity out of view but inside its radius is kept, not reloaded", () => {
        const entries = [region("m1", [placedAtXy("m1", 0, -500, 0)])];
        const decision = streamingDecision([0, 0, 0], entries, new Set(["m1:0"]), new Set(), 4, lookingAlongX);
        expect(decision.unload).toEqual([]);
        expect(decision.load).toEqual([]);
      });

      test("a waiting request goes stale once its entity is off view and past the cut", () => {
        const box = { extentsMin: [0, 0, 0], extentsMax: [100, 100, 100] } as const;
        const behindFar = placedAtXy("prop_building_01", 0, -500, 0);
        const behindNear = placedAtXy("prop_building_01", 1, -300, 0);
        const ahead = placedAtXy("prop_building_01", 2, 500, 0);
        expect(stillStreamWanted([0, 0, 0], box, behindFar, lookingAlongX)).toBe(false);
        expect(stillStreamWanted([0, 0, 0], box, behindNear, lookingAlongX)).toBe(true);
        expect(stillStreamWanted([0, 0, 0], box, ahead, lookingAlongX)).toBe(true);
        // Without a view the sphere alone decides, as before.
        expect(stillStreamWanted([0, 0, 0], box, behindFar)).toBe(true);
      });
    });

    /** The view decides order, never what unloads — otherwise turning the
     *  camera would tear out everything it stopped looking at. */
    test("does not unload what the camera turned away from", () => {
      const entries = [region("m1", [placedAtXy("m1", 0, -300, 0)])];
      const decision = streamingDecision(
        [0, 0, 0],
        entries,
        new Set(["m1:0"]),
        new Set(),
        4,
        lookingAlongX,
      );

      expect(decision.unload).toEqual([]);
    });
  });
});

describe("history", () => {
  test("committing appends and clears redo", () => {
    const undo = commitHistory([], step("a", editOfPlace(0), editOfPlace(1)));
    const undone = popHistory(undo);
    expect(undone.entry?.after.position).toEqual([1, 0, 0]);
    expect(undone.stack).toHaveLength(0);
  });

  test("popping an empty stack yields nothing", () => {
    expect(popHistory([])).toEqual({ stack: [], entry: null });
  });
});

/** A one-axis move for building history fixtures. */
function editOfPlace(x: number): TransformEdit {
  return { position: [x, 0, 0], rotation: [0, 0, 0, 1], scaleXy: 1, scaleZ: 1 };
}

/** A history entry fixture. */
function step(key: string, before: TransformEdit, after: TransformEdit): {
  key: string;
  before: TransformEdit;
  after: TransformEdit;
} {
  return { key, before, after };
}

/** A plausible placed entity, for the streamer tests. */
const entryFixture: Placed = {
  index: 0,
  name: "x",
  nameHash: 0,
  isMlo: false,
  fromMlo: false,
  position: [0, 0, 0],
  rotation: [0, 0, 0, 1],
  scaleXy: 1,
  scaleZ: 1,
  parentIndex: -1,
  lodLevel: "HD",
  lodDist: 100,
  children: 0,
};

/** A placed entity at a given position, for the streamer tests. */
function placedAt(name: string, x: number, y: number, index = 0): Placed {
  return { ...entryFixture, name, index, position: [x, y, 0] };
}

/** The same, with the entity index up front — the view tests need distinct ones. */
function placedAtXy(name: string, index: number, x: number, y: number): Placed {
  return { ...entryFixture, name, index, position: [x, y, 0] };
}

/** A placed map with a small streaming box around the origin. */
function regionEntryOf(key: string, placements: Placed[]): RegionEntry {
  return {
    key,
    extentsMin: [0, 0, 0],
    extentsMax: [100, 100, 100],
    placements: columnsFromPlaced(placements),
  };
}

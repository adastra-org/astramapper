import { describe, expect, test } from "bun:test";

import { decodeCellGrid, type CellGrid } from "./cellData";
import { gridBuffer } from "./cellFixtures";
import { clusterRows, leavesByCluster, type ClusterProgress } from "./clusters";

const sizes = [800, 400];
const clusters = [
  { key: "city", name: "City", leaves: 2, placements: 90, models: 12 },
  { key: "north", name: "North", leaves: 1, placements: 5, models: 3 },
  { key: "countryside", name: "Countryside", leaves: 0, placements: 0, models: 0 },
];

function grid(): CellGrid {
  return decodeCellGrid(
    gridBuffer(
      sizes,
      [
        { level: 1, q: 0, r: 0, count: 40 },
        { level: 1, q: 1, r: 0, count: 50 },
        { level: 0, q: 9, r: 9, count: 5 },
      ],
      [],
      { clusters, leafCluster: [0, 0, 1] },
    ),
  );
}

describe("the partition's zone clusters", () => {
  test("each leaf comes back with its cluster and the clusters with their numbers", () => {
    const g = grid();
    expect(g.clusters).toEqual(clusters);
    expect(Array.from(g.leafCluster)).toEqual([0, 0, 1]);
  });

  test("a backend without clusters leaves every leaf in none", () => {
    const g = decodeCellGrid(gridBuffer(sizes, [{ level: 0, q: 0, r: 0, count: 1 }]));
    expect(g.clusters).toEqual([]);
    expect(Array.from(g.leafCluster)).toEqual([0]);
  });

  test("a leaf pointing past the listed clusters is refused", () => {
    expect(() =>
      decodeCellGrid(
        gridBuffer(sizes, [{ level: 0, q: 0, r: 0, count: 1 }], [], {
          clusters,
          leafCluster: [7],
        }),
      ),
    ).toThrow();
  });

  test("a cluster column of the wrong length is refused", () => {
    expect(() =>
      decodeCellGrid(
        gridBuffer(sizes, [{ level: 0, q: 0, r: 0, count: 1 }], [], {
          clusters,
          leafCluster: [0, 1],
        }),
      ),
    ).toThrow();
  });
});

describe("leavesByCluster", () => {
  test("groups the leaves under their cluster, in leaf order", () => {
    const g = grid();
    const by = leavesByCluster(g);
    expect(by).toHaveLength(3);
    expect(by[0]!.map((l) => l.count)).toEqual([40, 50]);
    expect(by[1]!.map((l) => l.count)).toEqual([5]);
    expect(by[2]).toEqual([]);
  });
});

describe("clusterRows", () => {
  const progress: ClusterProgress[] = [
    { key: "city", heldLeaves: 1, leaves: 2, heldPlacements: 40, placements: 90, pinned: true },
    { key: "north", heldLeaves: 0, leaves: 1, heldPlacements: 0, placements: 5, pinned: false },
    { key: "countryside", heldLeaves: 0, leaves: 0, heldPlacements: 0, placements: 0, pinned: false },
  ];

  test("joins the numbers with what is held and what was measured", () => {
    const rows = clusterRows(grid(), progress, new Map([["city", 3 * 1024 ** 3]]));
    expect(rows.map((r) => r.key)).toEqual(["city", "north"]);
    expect(rows[0]).toMatchObject({
      name: "City",
      leaves: 2,
      placements: 90,
      models: 12,
      heldLeaves: 1,
      percent: 50,
      pinned: true,
      complete: false,
      modelBytes: 3 * 1024 ** 3,
    });
    expect(rows[1]).toMatchObject({ percent: 0, pinned: false, modelBytes: null });
  });

  test("an empty cluster gets no row", () => {
    const rows = clusterRows(grid(), progress, new Map());
    expect(rows.some((r) => r.key === "countryside")).toBe(false);
  });

  test("a cluster with every leaf held is complete", () => {
    const done: ClusterProgress[] = [
      { ...progress[0]!, heldLeaves: 2, heldPlacements: 90 },
      progress[1]!,
      progress[2]!,
    ];
    expect(clusterRows(grid(), done, new Map())[0]).toMatchObject({ percent: 100, complete: true });
  });
});

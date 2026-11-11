import { scenario } from "../scenario";

/** About 35 km over the whole map at 150 m/s: a few minutes. */
const TOUR = [
  "maze-bank-tower",
  "sandy-shores",
  "grapeseed",
  "paleto-bay",
  "fort-zancudo",
  "ls-airport",
  "legion-square",
];

export default scenario({
  name: "heap-flight",
  description:
    "Tours the map at 150 m/s for a few minutes and checks that the JS heap does not climb for the whole flight: a ceiling, and the second half of the flight no higher than 20% over the first.",
  steps: [
    { do: "openWorld" },
    { do: "go", to: "legion-square", lookAt: "maze-bank-tower" },
    { do: "waitFor", gauge: "cells held", op: ">", value: 0, timeoutMs: 120_000 },
    { do: "wait", ms: 5_000 },
    { do: "resetPerf" },
    { do: "fly", path: TOUR, speed: 150, wait: false },
    {
      do: "sample",
      name: "tour",
      forMs: 420_000,
      everyMs: 2_000,
      untilIdle: true,
      gauges: ["JS heap used (MB)", "resident models", "geometries", "textures", "cells held"],
    },
    { do: "shot", name: "after-tour" },
    { do: "assert", kind: "series", sample: "tour", key: "JS heap used (MB)", stat: "max", op: "<", value: 3_000 },
    // Still climbing in the second half means it is not bounded, whatever the ceiling.
    { do: "assert", kind: "series", sample: "tour", key: "JS heap used (MB)", stat: "halfRatio", op: "<=", value: 1.2 },
    { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
  ],
});

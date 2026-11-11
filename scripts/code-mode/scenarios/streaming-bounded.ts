import { scenario } from "../scenario";

/** Waypoints of a loop over the city, about 8.5 km. */
const LOOP = ["ls-airport", "del-perro-pier", "vinewood-sign", "legion-square"];

export default scenario({
  name: "streaming-bounded",
  description:
    "Flies a loop over Los Santos at 120 m/s and checks that cell streaming holds a bounded working set: cells and placements stay under a ceiling, the model cache stays near its budget, and nothing is logged as an error.",
  steps: [
    { do: "openWorld" },
    { do: "go", to: "legion-square", lookAt: "ls-airport" },
    { do: "waitFor", gauge: "cells held", op: ">", value: 0, timeoutMs: 120_000 },
    { do: "wait", ms: 5_000 },
    { do: "resetPerf" },
    { do: "fly", path: LOOP, speed: 120, wait: false },
    {
      do: "sample",
      name: "flight",
      forMs: 240_000,
      everyMs: 1_000,
      untilIdle: true,
      gauges: [
        "cells held",
        "cell placements held",
        "cells pending",
        "model cache (MB)",
        "model cache budget (MB)",
        "JS heap used (MB)",
      ],
      spans: [{ name: "frame", stats: ["p50", "p95"] }],
    },
    { do: "shot", name: "after-flight" },
    // Always something held: a flight over the city never leaves the working set empty.
    { do: "assert", kind: "series", sample: "flight", key: "cells held", stat: "min", op: ">", value: 0 },
    // Measured 20 to 75 cells and under 35,000 placements on the first flight (docs/debug-bridge.md).
    { do: "assert", kind: "series", sample: "flight", key: "cells held", stat: "max", op: "<=", value: 200 },
    {
      do: "assert",
      kind: "series",
      sample: "flight",
      key: "cell placements held",
      stat: "max",
      op: "<=",
      value: 100_000,
    },
    // The cache may overshoot its budget a little between passes.
    {
      do: "assert",
      kind: "series",
      sample: "flight",
      key: "model cache (MB)",
      stat: "max",
      op: "<=",
      value: { key: "model cache budget (MB)", factor: 1.1 },
    },
    { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
  ],
});

import { scenario } from "../scenario";

// Level with the camera, so the horizon is the middle of the screen. The band
// reaches from above the horizon (where distant mountains stand) to just under
// it. Below the horizon the scene has a grid helper, a little lighter than the
// clear colour, so a plain "differs from the background" test would count the
// grid as ground; the tolerance of 60 per channel counts only real terrain.
const BAND = [0, 0.35, 1, 0.6] as const;

export default scenario({
  name: "terrain-distance",
  description:
    "From a high point over the city, looks level toward Sandy Shores and Paleto Bay and checks that terrain is drawn far beyond the streamed cells (not just sky and the grid helper). Screenshots are kept for a look.",
  steps: [
    { do: "openWorld" },
    { do: "go", to: [-75, -818, 360], lookAt: [1850, 3700, 360] },
    { do: "waitFor", gauge: "cells held", op: ">", value: 0, timeoutMs: 120_000 },
    { do: "wait", ms: 10_000 },
    {
      do: "sample",
      name: "toward-sandy-shores",
      forMs: 3_000,
      gauges: ["cell working set reach (m)", "cells held", "triangles", "draw calls"],
    },
    { do: "shot", name: "toward-sandy-shores" },
    { do: "go", to: [-75, -818, 360], lookAt: [-150, 6200, 360] },
    { do: "wait", ms: 10_000 },
    { do: "shot", name: "toward-paleto-bay" },
    {
      do: "assert",
      kind: "image",
      shot: "toward-sandy-shores",
      region: BAND,
      tolerance: 60,
      op: ">=",
      value: 0.05,
      label: "distant terrain drawn toward Sandy Shores",
    },
    {
      do: "assert",
      kind: "image",
      shot: "toward-paleto-bay",
      region: BAND,
      tolerance: 60,
      op: ">=",
      value: 0.05,
      label: "distant terrain drawn toward Paleto Bay",
    },
    { do: "assert", kind: "gauge", gauge: "triangles", op: ">", value: 0 },
    { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
  ],
});

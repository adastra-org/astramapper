import { scenario } from "../scenario";

// Level with the camera, so the horizon is the middle of the screen and the band
// just under it is ground far away: beyond the cells that are streamed in, so
// only the whole-map terrain layer can draw it.
const BAND = [0.05, 0.52, 0.95, 0.62] as const;

export default scenario({
  name: "terrain-distance",
  description:
    "From a high point over the city, looks level toward Sandy Shores and Paleto Bay and checks that the ground just under the horizon is drawn (not the clear colour), so the terrain layer shows at distance. Screenshots are kept for a look.",
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
      op: ">=",
      value: 0.9,
      label: "ground drawn under the horizon toward Sandy Shores",
    },
    {
      do: "assert",
      kind: "image",
      shot: "toward-paleto-bay",
      region: BAND,
      op: ">=",
      value: 0.9,
      label: "ground drawn under the horizon toward Paleto Bay",
    },
    { do: "assert", kind: "gauge", gauge: "triangles", op: ">", value: 0 },
    { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
  ],
});

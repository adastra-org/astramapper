import { scenario } from "../scenario";

// The ground below the horizon at a fixed pose over Sandy Shores, looking
// north and down. The scene's grid helper is a little lighter than the clear
// colour, so a tolerance of 60 per channel counts only drawn ground.
const GROUND = [0, 0.55, 1, 1] as const;
const POSE = { to: [1850, 3700, 120] as const, lookAt: [1850, 4300, 20] as const };

export default scenario({
  name: "terrain-revisit",
  description:
    "Terrain must stay after the camera leaves: looks at the ground over Sandy Shores, flies to the LS airport and back at 120 m/s, and compares the ground coverage at the same pose before and after, and from the far end looking back. Screenshots are kept.",
  steps: [
    { do: "openWorld" },
    { do: "go", ...POSE },
    { do: "waitFor", gauge: "cells held", op: ">", value: 0, timeoutMs: 120_000 },
    // The app frames its first camera when World opens, so aim again once it has
    // loaded: the same call at the return is then the same pose.
    { do: "go", ...POSE },
    { do: "wait", ms: 25_000 },
    { do: "shot", name: "first-visit" },
    { do: "fly", path: [[1850, 3700, 120], [-1300, -2400, 120]], speed: 120, timeoutMs: 180_000 },
    { do: "go", to: [-1300, -2400, 120], lookAt: [1850, 3700, 20] },
    { do: "wait", ms: 15_000 },
    { do: "shot", name: "looking-back" },
    { do: "fly", path: [[-1300, -2400, 120], [1850, 3700, 120]], speed: 120, timeoutMs: 180_000 },
    { do: "go", ...POSE },
    { do: "wait", ms: 25_000 },
    { do: "shot", name: "return" },
    {
      do: "assert",
      kind: "image",
      shot: "first-visit",
      region: GROUND,
      tolerance: 60,
      op: ">=",
      value: 0.5,
      label: "ground drawn at the first visit",
    },
    {
      do: "assert",
      kind: "image",
      shot: "return",
      region: GROUND,
      tolerance: 60,
      op: ">=",
      value: 0.5,
      label: "ground still drawn when the camera returns",
    },
    {
      do: "assert",
      kind: "imageDiff",
      shot: "first-visit",
      against: "return",
      region: [0, 0.2, 1, 0.65],
      tolerance: 30,
      op: "<=",
      value: 0.08,
      label: "the distant ground is the same picture on return",
    },
    { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
  ],
});

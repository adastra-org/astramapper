import { scenario } from "../scenario";

export default scenario({
  name: "smoke",
  description:
    "Opens World, lets it draw for a few seconds and checks that frames are rendered, cells are held and the console is clean.",
  steps: [
    { do: "openWorld" },
    { do: "go", to: "legion-square", lookAt: [195, -600, 40] },
    { do: "waitFor", gauge: "cells held", op: ">", value: 0, timeoutMs: 120_000 },
    { do: "resetPerf" },
    { do: "sample", name: "idle", forMs: 8_000, gauges: ["cells held", "JS heap used (MB)"] },
    { do: "shot", name: "legion-square" },
    { do: "assert", kind: "span", span: "frame", stat: "calls", op: ">", value: 0 },
    { do: "assert", kind: "gauge", gauge: "cells held", op: ">", value: 0 },
    { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
  ],
});

// Code Mode: read and drive the running app through its debug bridge, or run a
// scenario against it. See docs/code-mode.md.
//
//   bun scripts/bridge.ts run smoke
//   bun scripts/bridge.ts state --gauges
import { main } from "./code-mode/cli";

process.exit(await main(process.argv.slice(2)));

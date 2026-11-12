# Documentation

| Document | What it covers |
|---|---|
| [architecture.md](architecture.md) | Why things are shaped the way they are |
| [phases.md](phases.md) | Roadmap, where the port stands, what to do next |
| [porting-guide.md](porting-guide.md) | How to port one unit |
| [formats.md](formats.md) | Per-format status |
| [streaming.md](streaming.md) | How World loads models, instanced drawing, and what was measured |
| [../scripts/ipc-bench.ts](../scripts/ipc-bench.ts) | Raw IPC cost benchmark (`bun scripts/ipc-bench.ts`), see streaming.md |
| [hex-streaming.md](hex-streaming.md) | Design for loading by hex cells |
| [debug-bridge.md](debug-bridge.md) | Read the console and F3 and drive the camera from a shell |
| [vehicle-tools.md](vehicle-tools.md) | Vehicle audit and mesh optimizer CLIs: damaged files, YTD over 16 MB, LOD generation, backup and rollback |
| [Vehicle findings (astra-dev)](https://github.com/adastra-org/astra-dev/blob/dev/docs/vehiculos/README.md) | Flags checked against the official list, glass flag vs collision material, material table, shaders, tint and LOD status. Development docs live in astra-dev |
| [code-mode.md](code-mode.md) | Scenario runner on the bridge: `bun scripts/bridge.ts run <scenario>` |
| [fixes-2026-11-11-v0.3.5/](fixes-2026-11-11-v0.3.5/README.md) | Terrain, placement, trees and LOD problems seen in the app, with status |
| [fixes-2026-11-11-v0.3.0/](fixes-2026-11-11-v0.3.0/README.md) | Zone clusters, with status |
| [fixes-2026-11-11-v0.1.0/](fixes-2026-11-11-v0.1.0/README.md) | Corrections and improvements batch, with status |

Long-term items live as GitHub issues in this repo and on the Ad Astra Roadmap.

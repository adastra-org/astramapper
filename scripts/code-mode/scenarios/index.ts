import type { Scenario } from "../scenario";
import heapFlight from "./heap-flight";
import smoke from "./smoke";
import streamingBounded from "./streaming-bounded";
import terrainDistance from "./terrain-distance";
import terrainRevisit from "./terrain-revisit";

/** The scenarios that ship with the repo, by name. */
export const SCENARIOS: Readonly<Record<string, Scenario>> = Object.fromEntries(
  [smoke, streamingBounded, terrainDistance, terrainRevisit, heapFlight].map((s) => [s.name, s]),
);

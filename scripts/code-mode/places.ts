/**
 * Named camera positions in game coordinates (X east, Y north, Z up), the ones
 * the app prints. `z` is a viewing altitude, not the ground height: it puts the
 * camera a hundred metres or so above the roofs. Approximate: they are meant to
 * put the camera over the right district, not at a surveyed point.
 */
import type { Vec3 } from "./types";

export const PLACES: Readonly<Record<string, Vec3>> = {
  "legion-square": [195, -935, 60],
  "maze-bank-tower": [-75, -818, 360],
  "vinewood-sign": [720, 1210, 260],
  "ls-airport": [-1300, -2400, 80],
  "del-perro-pier": [-1850, -1230, 60],
  "fort-zancudo": [-2100, 3100, 80],
  "sandy-shores": [1850, 3700, 90],
  grapeseed: [1700, 4900, 90],
  "paleto-bay": [-150, 6200, 90],
  "mount-chiliad": [450, 5570, 900],
};

/** A place name or an explicit `[x, y, z]`. */
export type Place = string | Vec3;

export function resolvePlace(place: Place): Vec3 {
  if (typeof place !== "string") return place;
  const found = PLACES[place.toLowerCase().replace(/[\s_]+/g, "-")];
  if (!found) {
    throw new Error(`unknown place "${place}"; known: ${Object.keys(PLACES).join(", ")}`);
  }
  return found;
}

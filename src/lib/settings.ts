/**
 * Persisted app settings, backed by the Tauri store plugin. `settings.json`
 * lives next to the app's other config, not the game folder itself.
 */
import { LazyStore } from "@tauri-apps/plugin-store";

const store = new LazyStore("settings.json");

export function getLastGameFolder(): Promise<string | undefined> {
  return store.get<string>("lastGameFolder");
}

export async function setLastGameFolder(folder: string): Promise<void> {
  await store.set("lastGameFolder", folder);
  await store.save();
}

/** What the user chose at the precompile prompt for an install, if they did. */
export type BakeOnboarding = "baked" | "skipped";

/** Keyed by install stamp, so a game update asks again. */
export async function getBakeOnboarding(stamp: string): Promise<BakeOnboarding | undefined> {
  return store.get<BakeOnboarding>(`bakeOnboarding:${stamp}`);
}

export async function setBakeOnboarding(stamp: string, answer: BakeOnboarding): Promise<void> {
  await store.set(`bakeOnboarding:${stamp}`, answer);
  await store.save();
}

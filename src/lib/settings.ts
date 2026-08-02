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

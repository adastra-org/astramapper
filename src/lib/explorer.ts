/**
 * Typed wrappers over the Explorer IPC commands in `src-tauri/src/explorer.rs`.
 *
 * These types mirror the Rust structs exactly. The backend serializes camelCase,
 * so the shapes here are the wire format, not a translation of it.
 */
import { invoke } from "@tauri-apps/api/core";

/** What an entry is. Mirrors the `kind` field the backend computes. */
export type EntryKind = "directory" | "archive" | "binary" | "resource";

/** One row of a directory listing. */
export type Listing = {
  readonly name: string;
  /** Full path, and the handle for listing this entry. */
  readonly path: string;
  readonly kind: EntryKind;
  /** Size on disk in bytes. Zero for directories. */
  readonly size: number;
  /** Whether listing this entry yields anything. */
  readonly expandable: boolean;
};

/** What opening a game folder reports back. */
export type GameSummary = {
  readonly folder: string;
  readonly topLevelArchives: number;
  readonly totalArchives: number;
  readonly totalEntries: number;
  /** Archives that failed to scan. One bad archive does not fail the folder. */
  readonly failed: readonly string[];
  readonly elapsedMs: number;
};

export function openGameFolder(folder: string): Promise<GameSummary> {
  return invoke<GameSummary>("open_game_folder", { folder });
}

/** List a directory. An empty path lists the top-level archives. */
export function listDir(path: string): Promise<Listing[]> {
  return invoke<Listing[]>("list_dir", { path: path === "" ? null : path });
}

export function gameIsOpen(): Promise<boolean> {
  return invoke<boolean>("game_is_open");
}

/**
 * Split an archive path into its clickable segments.
 *
 * Paths are backslash-separated and each prefix is itself listable, so the
 * breadcrumb is just the running prefixes.
 */
export function breadcrumb(path: string): readonly { label: string; path: string }[] {
  if (path === "") return [];
  const parts = path.split("\\").filter((p) => p !== "");
  return parts.map((label, i) => ({
    label,
    path: parts.slice(0, i + 1).join("\\"),
  }));
}

/** The parent of an archive path, or "" at the top level. */
export function parentOf(path: string): string {
  const cut = path.lastIndexOf("\\");
  return cut < 0 ? "" : path.slice(0, cut);
}

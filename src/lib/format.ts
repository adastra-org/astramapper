/** Formatting helpers shared by the explorer, hex viewer and hash search. */

/**
 * Render a Jenkins hash the way CodeWalker does: `0x` followed by eight
 * uppercase hex digits.
 *
 * Hashes cross the IPC boundary as JSON numbers. A `u32` is within the range
 * JSON numbers represent exactly, but a negative value would mean the backend
 * sent an `i32` somewhere, so coerce through `>>> 0` rather than silently
 * printing a `-`.
 */
export function formatHash(hash: number): string {
  return `0x${(hash >>> 0).toString(16).toUpperCase().padStart(8, "0")}`;
}

/** Byte counts for archive listings: `1.4 GB`, `26.4 MB`, `512 B`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

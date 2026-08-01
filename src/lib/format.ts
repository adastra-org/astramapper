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

/** One row of a hex dump. */
export type HexLine = {
  /** Absolute offset in the file, not in the page. */
  readonly offset: number;
  /** Space-separated pairs, padded so short final rows still align. */
  readonly hex: string;
  /** Printable ASCII, everything else a dot. */
  readonly ascii: string;
};

/** How many bytes one hex row shows. */
export const HEX_ROW = 16;

/**
 * Lay a page of bytes out as hex rows.
 *
 * `baseOffset` is where the page starts in the file, so the offsets read as
 * file offsets rather than page offsets — which is the whole point of paging
 * through a 20 MB file sixteen bytes at a time.
 */
export function hexLines(bytes: Uint8Array, baseOffset: number): HexLine[] {
  const lines: HexLine[] = [];
  for (let start = 0; start < bytes.length; start += HEX_ROW) {
    const row = bytes.subarray(start, start + HEX_ROW);
    const hex = Array.from(row, (b) => b.toString(16).padStart(2, "0")).join(" ");
    const ascii = Array.from(row, (b) =>
      b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".",
    ).join("");
    lines.push({
      offset: baseOffset + start,
      // Pad so a short final row keeps the ASCII column where it was.
      hex: hex.padEnd(HEX_ROW * 3 - 1, " "),
      ascii,
    });
  }
  return lines;
}

/** A file offset as eight uppercase hex digits, the way a hex editor shows it. */
export function formatOffset(offset: number): string {
  return offset.toString(16).toUpperCase().padStart(8, "0");
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

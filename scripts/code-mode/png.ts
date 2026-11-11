/**
 * A just-enough PNG reader for screenshot assertions: 8-bit, non-interlaced
 * gray, gray+alpha, RGB or RGBA, which is what `canvas.toDataURL` produces.
 */
import { inflateSync } from "node:zlib";

export type Image = { width: number; height: number; channels: number; data: Uint8Array };

const SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

const CHANNELS: Record<number, number> = { 0: 1, 2: 3, 4: 2, 6: 4 };

export function decodePng(bytes: Uint8Array): Image {
  if (SIGNATURE.some((b, i) => bytes[i] !== b)) throw new Error("not a PNG");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let width = 0;
  let height = 0;
  let channels = 0;
  const idat: Uint8Array[] = [];
  let offset = 8;
  while (offset + 8 <= bytes.length) {
    const length = view.getUint32(offset);
    const type = String.fromCharCode(...bytes.subarray(offset + 4, offset + 8));
    const body = bytes.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      width = view.getUint32(offset + 8);
      height = view.getUint32(offset + 12);
      const depth = body[8];
      const colorType = body[9] ?? -1;
      const interlace = body[12];
      if (depth !== 8) throw new Error(`unsupported PNG bit depth ${depth}`);
      if (interlace !== 0) throw new Error("interlaced PNG is not supported");
      channels = CHANNELS[colorType] ?? 0;
      if (channels === 0) throw new Error(`unsupported PNG colour type ${colorType}`);
    } else if (type === "IDAT") {
      idat.push(body);
    } else if (type === "IEND") {
      break;
    }
    offset += 12 + length;
  }
  if (width === 0 || height === 0) throw new Error("PNG has no IHDR");
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const out = new Uint8Array(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)]!;
    const src = y * (stride + 1) + 1;
    const row = y * stride;
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? out[row + x - channels]! : 0;
      const up = y > 0 ? out[row - stride + x]! : 0;
      const upLeft = y > 0 && x >= channels ? out[row - stride + x - channels]! : 0;
      const value = raw[src + x]!;
      let predicted = 0;
      switch (filter) {
        case 0:
          break;
        case 1:
          predicted = left;
          break;
        case 2:
          predicted = up;
          break;
        case 3:
          predicted = (left + up) >> 1;
          break;
        case 4: {
          const p = left + up - upLeft;
          const pa = Math.abs(p - left);
          const pb = Math.abs(p - up);
          const pc = Math.abs(p - upLeft);
          predicted = pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
          break;
        }
        default:
          throw new Error(`bad PNG filter ${filter}`);
      }
      out[row + x] = (value + predicted) & 0xff;
    }
  }
  return { width, height, channels, data: out };
}

/** RGB of the pixel at (x, y); gray expands to all three. */
export function pixel(image: Image, x: number, y: number): [number, number, number] {
  const i = (y * image.width + x) * image.channels;
  if (image.channels < 3) return [image.data[i]!, image.data[i]!, image.data[i]!];
  return [image.data[i]!, image.data[i + 1]!, image.data[i + 2]!];
}

export type Region = readonly [number, number, number, number];

/**
 * The fraction of pixels in `region` (x0, y0, x1, y1 as fractions of the image)
 * whose colour is further than `tolerance` (per channel) from `background`.
 */
export function coverage(
  image: Image,
  region: Region,
  background: readonly [number, number, number],
  tolerance = 6,
): number {
  const x0 = Math.floor(region[0] * image.width);
  const y0 = Math.floor(region[1] * image.height);
  const x1 = Math.min(image.width, Math.ceil(region[2] * image.width));
  const y1 = Math.min(image.height, Math.ceil(region[3] * image.height));
  let total = 0;
  let different = 0;
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      const [r, g, b] = pixel(image, x, y);
      total++;
      if (
        Math.abs(r - background[0]) > tolerance ||
        Math.abs(g - background[1]) > tolerance ||
        Math.abs(b - background[2]) > tolerance
      ) {
        different++;
      }
    }
  }
  return total === 0 ? Number.NaN : different / total;
}

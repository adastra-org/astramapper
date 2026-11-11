import { describe, expect, test } from "bun:test";

import { encodePng, solidPng } from "./fake-app";
import { coverage, decodePng, pixel } from "./png";

const gradient = (w: number, h: number): Array<[number, number, number]> =>
  Array.from({ length: w * h }, (_, i) => {
    const x = i % w;
    const y = Math.floor(i / w);
    return [(x * 37 + y * 11) & 255, (x * 5 + y * 91) & 255, (x * y * 3) & 255];
  });

describe("decodePng", () => {
  test("every row filter decodes back to the pixels", () => {
    const pixels = gradient(9, 10);
    for (const filters of [[0], [1], [2], [3], [4], [0, 1, 2, 3, 4]]) {
      const image = decodePng(encodePng(9, 10, pixels, filters));
      expect(image.width).toBe(9);
      expect(image.height).toBe(10);
      for (let y = 0; y < 10; y++) {
        for (let x = 0; x < 9; x++) expect(pixel(image, x, y)).toEqual(pixels[y * 9 + x]!);
      }
    }
  });

  test("things that are not a PNG are refused", () => {
    expect(() => decodePng(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9]))).toThrow("not a PNG");
  });
});

describe("coverage", () => {
  // Left half the background, right half a different colour.
  const pixels: Array<[number, number, number]> = [];
  for (let y = 0; y < 4; y++) {
    for (let x = 0; x < 8; x++) pixels.push(x < 4 ? [16, 20, 28] : [200, 100, 50]);
  }
  const image = decodePng(encodePng(8, 4, pixels));

  test("the fraction of a region that differs from the background", () => {
    expect(coverage(image, [0, 0, 1, 1], [16, 20, 28])).toBeCloseTo(0.5);
    expect(coverage(image, [0, 0, 0.5, 1], [16, 20, 28])).toBe(0);
    expect(coverage(image, [0.5, 0, 1, 1], [16, 20, 28])).toBe(1);
  });

  test("a small colour difference is within tolerance", () => {
    const near = decodePng(solidPng(2, 2, [18, 22, 30]));
    expect(coverage(near, [0, 0, 1, 1], [16, 20, 28])).toBe(0);
    expect(coverage(near, [0, 0, 1, 1], [16, 20, 28], 1)).toBe(1);
  });

  test("an empty region is NaN", () => {
    expect(coverage(image, [0.5, 0.5, 0.5, 0.5], [0, 0, 0])).toBeNaN();
  });
});

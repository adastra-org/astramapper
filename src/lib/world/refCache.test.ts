import { describe, expect, test } from "bun:test";

import { RefCache } from "./refCache";

function counted(): { value: { disposed: number; dispose(): void }; made: number } {
  const value = {
    disposed: 0,
    dispose() {
      this.disposed += 1;
    },
  };
  return { value, made: 0 };
}

describe("RefCache", () => {
  test("a second acquire shares the value and does not make another", () => {
    const cache = new RefCache<{ disposed: number; dispose(): void }>();
    let made = 0;
    const make = () => {
      made += 1;
      return counted().value;
    };
    const a = cache.acquire("k", make);
    const b = cache.acquire("k", make);
    expect(a).toBe(b);
    expect(made).toBe(1);
    expect(cache.size).toBe(1);
  });

  test("the last release disposes once and forgets the key", () => {
    const cache = new RefCache<{ disposed: number; dispose(): void }>();
    const value = counted().value;
    cache.acquire("k", () => value);
    cache.acquire("k", () => value);
    cache.release("k");
    expect(value.disposed).toBe(0);
    cache.release("k");
    expect(value.disposed).toBe(1);
    expect(cache.size).toBe(0);
  });

  test("a key acquired again after its last release is made fresh", () => {
    const cache = new RefCache<{ disposed: number; dispose(): void }>();
    const first = counted().value;
    const second = counted().value;
    cache.acquire("k", () => first);
    cache.release("k");
    expect(cache.acquire("k", () => second)).toBe(second);
  });

  test("releasing an unknown key or too often does nothing", () => {
    const cache = new RefCache<{ disposed: number; dispose(): void }>();
    const value = counted().value;
    cache.release("missing");
    cache.acquire("k", () => value);
    cache.release("k");
    cache.release("k");
    expect(value.disposed).toBe(1);
  });

  test("clear disposes every value whatever its users", () => {
    const cache = new RefCache<{ disposed: number; dispose(): void }>();
    const a = counted().value;
    const b = counted().value;
    cache.acquire("a", () => a);
    cache.acquire("a", () => a);
    cache.acquire("b", () => b);
    cache.clear();
    expect([a.disposed, b.disposed]).toEqual([1, 1]);
    expect(cache.size).toBe(0);
  });
});

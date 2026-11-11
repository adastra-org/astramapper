import { describe, expect, test } from "bun:test";

import { throttleSubscribe, type ThrottleClock } from "./throttle";

/** A clock the test advances by hand. */
function fakeClock() {
  let now = 1000;
  let nextId = 1;
  const timers = new Map<number, { at: number; run: () => void }>();
  const clock: ThrottleClock = {
    now: () => now,
    setTimer: (run, ms) => {
      const id = nextId++;
      timers.set(id, { at: now + ms, run });
      return id;
    },
    clearTimer: (handle) => {
      timers.delete(handle as number);
    },
  };
  return {
    clock,
    advance(ms: number) {
      now += ms;
      for (const [id, timer] of timers) {
        if (timer.at > now) continue;
        timers.delete(id);
        timer.run();
      }
    },
    pending: () => timers.size,
  };
}

/** A store that only knows how to notify. */
function store() {
  const listeners = new Set<() => void>();
  return {
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    notify: () => {
      for (const listener of listeners) listener();
    },
    size: () => listeners.size,
  };
}

describe("throttleSubscribe", () => {
  test("the first notification goes through at once", () => {
    const source = store();
    const time = fakeClock();
    let calls = 0;
    throttleSubscribe(source.subscribe, 250, time.clock)(() => (calls += 1));
    source.notify();
    expect(calls).toBe(1);
  });

  test("a burst inside the interval becomes one trailing notification", () => {
    const source = store();
    const time = fakeClock();
    let calls = 0;
    throttleSubscribe(source.subscribe, 250, time.clock)(() => (calls += 1));
    source.notify();
    time.advance(10);
    for (let i = 0; i < 20; i += 1) source.notify();
    expect(calls).toBe(1);
    expect(time.pending()).toBe(1);
    time.advance(239);
    expect(calls).toBe(1);
    time.advance(1);
    expect(calls).toBe(2);
    expect(time.pending()).toBe(0);
  });

  test("goes through at once again after a quiet spell", () => {
    const source = store();
    const time = fakeClock();
    let calls = 0;
    throttleSubscribe(source.subscribe, 250, time.clock)(() => (calls += 1));
    source.notify();
    time.advance(1000);
    source.notify();
    expect(calls).toBe(2);
    expect(time.pending()).toBe(0);
  });

  test("never notifies more than once per interval over a steady stream", () => {
    const source = store();
    const time = fakeClock();
    let calls = 0;
    throttleSubscribe(source.subscribe, 100, time.clock)(() => (calls += 1));
    for (let step = 0; step < 100; step += 1) {
      source.notify();
      time.advance(16);
    }
    // 1,600 ms at one notification per 100 ms, the first free.
    expect(calls).toBeLessThanOrEqual(17);
    expect(calls).toBeGreaterThanOrEqual(15);
  });

  test("unsubscribing cancels the pending trailing notification", () => {
    const source = store();
    const time = fakeClock();
    let calls = 0;
    const off = throttleSubscribe(source.subscribe, 250, time.clock)(() => (calls += 1));
    source.notify();
    time.advance(10);
    source.notify();
    off();
    time.advance(1000);
    expect(calls).toBe(1);
    expect(source.size()).toBe(0);
    expect(time.pending()).toBe(0);
  });

  test("listeners are throttled independently", () => {
    const source = store();
    const time = fakeClock();
    const throttled = throttleSubscribe(source.subscribe, 250, time.clock);
    let a = 0;
    let b = 0;
    throttled(() => (a += 1));
    source.notify();
    time.advance(10);
    throttled(() => (b += 1));
    source.notify();
    expect(a).toBe(1);
    expect(b).toBe(1);
  });
});

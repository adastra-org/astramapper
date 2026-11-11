/**
 * A store's `subscribe` that tells each listener at most once per interval.
 *
 * # Why
 *
 * `useSyncExternalStore` re-renders its component synchronously for every
 * notification. World reads two stores that change several times a second while
 * the camera moves (the held cells, the loaded geometry), and every change
 * re-rendered the whole sidebar, re-merged the cells' columns and rebuilt the
 * placement index, in a microtask right behind whatever caused it: a streaming
 * pass, an IPC reply. Those showed up as 50 to 120 ms tasks. The sidebar does not
 * need to follow at that rate, so it follows at the interval's.
 *
 * The first notification after a quiet spell goes through at once, and any that
 * come inside the interval are folded into one trailing notification, so the last
 * change is never lost. `getSnapshot` is untouched: a render for another reason
 * still reads the newest value.
 */
export type Subscribe = (listener: () => void) => () => void;

/** Timers, injectable so the tests drive the clock. */
export type ThrottleClock = {
  readonly now: () => number;
  readonly setTimer: (run: () => void, ms: number) => unknown;
  readonly clearTimer: (handle: unknown) => void;
};

const realClock: ThrottleClock = {
  now: () => performance.now(),
  setTimer: (run, ms) => setTimeout(run, ms),
  clearTimer: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export function throttleSubscribe(
  subscribe: Subscribe,
  intervalMs: number,
  clock: ThrottleClock = realClock,
): Subscribe {
  return (listener) => {
    let last = Number.NEGATIVE_INFINITY;
    let timer: unknown = null;
    const fire = (): void => {
      timer = null;
      last = clock.now();
      listener();
    };
    const unsubscribe = subscribe(() => {
      if (timer !== null) return;
      const wait = intervalMs - (clock.now() - last);
      if (wait <= 0) fire();
      else timer = clock.setTimer(fire, wait);
    });
    return () => {
      unsubscribe();
      if (timer !== null) clock.clearTimer(timer);
      timer = null;
    };
  };
}

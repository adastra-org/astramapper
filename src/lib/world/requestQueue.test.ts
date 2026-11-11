import { describe, expect, test } from "bun:test";
import { QueueDropped, RequestQueue } from "./requestQueue";

/** A task the test finishes by hand. */
function manual<T>() {
  let finish!: (value: T) => void;
  const promise = new Promise<T>((resolve) => {
    finish = resolve;
  });
  return { promise, finish };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("RequestQueue", () => {
  test("never runs more than the limit at once", async () => {
    const queue = new RequestQueue<null>(4);
    let live = 0;
    let peak = 0;
    const gates = Array.from({ length: 12 }, () => manual<void>());
    const results = gates.map((gate, i) =>
      queue.enqueue(`k${i}`, i, null, async () => {
        live += 1;
        peak = Math.max(peak, live);
        await gate.promise;
        live -= 1;
        return i;
      }),
    );
    expect(queue.active).toBe(4);
    expect(queue.size).toBe(8);
    for (const gate of gates) {
      gate.finish();
      // oxlint-disable-next-line no-await-in-loop
      await tick();
    }
    expect(await Promise.all(results)).toEqual(gates.map((_, i) => i));
    expect(peak).toBe(4);
  });

  test("starts the lowest priority first, arrival order among equals", async () => {
    const queue = new RequestQueue<null>(1);
    const order: string[] = [];
    const first = manual<void>();
    const run = (key: string, priority: number, gate?: Promise<void>) =>
      queue.enqueue(key, priority, null, async () => {
        order.push(key);
        await gate;
      });
    const all = [
      run("blocker", 0, first.promise),
      run("far", 90),
      run("near", 5),
      run("tie-a", 20),
      run("tie-b", 20),
    ];
    first.finish();
    await Promise.all(all);
    expect(order).toEqual(["blocker", "near", "tie-a", "tie-b", "far"]);
  });

  test("reprioritize reorders waiting jobs and drops the unwanted ones", async () => {
    const queue = new RequestQueue<{ at: number }>(1);
    const order: string[] = [];
    const gate = manual<void>();
    const blocker = queue.enqueue("blocker", 0, { at: 0 }, () => gate.promise);
    const jobs = [10, 20, 30].map((at) =>
      queue.enqueue(`at${at}`, at, { at }, async () => {
        order.push(`at${at}`);
      }),
    );
    const settled = Promise.allSettled(jobs);

    // The camera moved: 30 is now nearest, 20 is out of range.
    const dropped = queue.reprioritize(({ data }) => (data.at === 20 ? null : 100 - data.at));
    expect(dropped).toBe(1);
    expect(queue.droppedCount).toBe(1);

    gate.finish();
    await blocker;
    const results = await settled;
    expect(results[1]!.status).toBe("rejected");
    expect((results[1] as PromiseRejectedResult).reason).toBeInstanceOf(QueueDropped);
    expect(order).toEqual(["at30", "at10"]);
  });

  test("a job that is not droppable survives reprioritize and trim", async () => {
    const queue = new RequestQueue<null>(1);
    const gate = manual<void>();
    const blocker = queue.enqueue("blocker", 0, null, () => gate.promise);
    const kept = queue.enqueue("click", -1, null, async () => "ok", false);
    expect(queue.reprioritize(() => null)).toBe(0);
    expect(queue.evictWorst()).toBe(false);
    gate.finish();
    await blocker;
    expect(await kept).toBe("ok");
  });

  test("evictWorst drops the highest priority number", async () => {
    const queue = new RequestQueue<null>(1);
    const gate = manual<void>();
    const blocker = queue.enqueue("blocker", 0, null, () => gate.promise);
    const near = queue.enqueue("near", 1, null, async () => "near");
    const far = queue.enqueue("far", 9, null, async () => "far");
    expect(queue.worstDroppable()).toBe(9);
    expect(queue.evictWorst()).toBe(true);
    await expect(far).rejects.toBeInstanceOf(QueueDropped);
    gate.finish();
    await blocker;
    expect(await near).toBe("near");
  });

  test("a failing or throwing task frees its slot", async () => {
    const queue = new RequestQueue<null>(1);
    const bad = queue.enqueue("bad", 0, null, () => {
      throw new Error("sync");
    });
    const good = queue.enqueue("good", 1, null, async () => "ok");
    await expect(bad).rejects.toThrow("sync");
    expect(await good).toBe("ok");
    expect(queue.active).toBe(0);
  });
});

/**
 * A bounded, re-rankable request queue.
 *
 * The streaming pass used to start up to 24 model loads every 250ms with
 * nothing counting how many were already out. Measured with F3: 30 `world_model`
 * calls in flight at the peak, a 659ms median round trip, and 0.07ms of backend
 * work per call — so the time was queueing behind the IPC, and the models the
 * camera was standing next to waited their turn behind ones it had since walked
 * away from.
 *
 * This holds the line instead: at most `limit` tasks run at once, the rest wait
 * here where they can still be reordered or dropped, and the next one to start
 * is always the best-ranked at that moment (lowest priority number first).
 */

/** What a dropped job's promise rejects with. Callers treat it as "never mind". */
export class QueueDropped extends Error {
  constructor(key: string) {
    super(`request ${key} was dropped from the queue`);
    this.name = "QueueDropped";
  }
}

type Job<D> = {
  readonly key: string;
  readonly data: D;
  /** Whether `reprioritize` / `trim` may drop it. A user's click is not. */
  readonly droppable: boolean;
  priority: number;
  readonly start: () => void;
  readonly drop: () => void;
};

export class RequestQueue<D = undefined> {
  private readonly waiting: Job<D>[] = [];
  private running = 0;
  private dropped = 0;

  constructor(
    private readonly limit: number,
    private readonly onChange?: (state: { active: number; waiting: number; dropped: number }) => void,
  ) {}

  get active(): number {
    return this.running;
  }

  get size(): number {
    return this.waiting.length;
  }

  /** Total jobs dropped before they ever ran. */
  get droppedCount(): number {
    return this.dropped;
  }

  /**
   * Queue `task`. It starts as soon as a slot is free and nothing better is
   * waiting. Lower `priority` runs first; ties run in arrival order.
   */
  enqueue<T>(
    key: string,
    priority: number,
    data: D,
    task: () => Promise<T>,
    droppable = true,
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const job: Job<D> = {
        key,
        data,
        droppable,
        priority,
        start: () => {
          this.running += 1;
          this.changed();
          // An async wrapper, so a task that throws before it returns a promise
          // still frees its slot.
          (async () => task())().then(resolve, reject).finally(() => {
            this.running -= 1;
            this.pump();
          });
        },
        drop: () => {
          this.dropped += 1;
          reject(new QueueDropped(key));
        },
      };
      this.waiting.push(job);
      this.pump();
      this.changed();
    });
  }

  /**
   * Re-rank the waiting jobs. `rank` returns a new priority, or `null` to say
   * the job is no longer wanted — which drops it if it is droppable and leaves
   * its priority alone if it is not. Running jobs are not touched.
   *
   * @returns how many jobs were dropped.
   */
  reprioritize(rank: (job: { key: string; data: D }) => number | null): number {
    let dropped = 0;
    const kept: Job<D>[] = [];
    for (const job of this.waiting) {
      if (!job.droppable) {
        kept.push(job);
        continue;
      }
      const next = rank(job);
      if (next === null) {
        job.drop();
        dropped += 1;
      } else {
        job.priority = next;
        kept.push(job);
      }
    }
    this.waiting.length = 0;
    this.waiting.push(...kept);
    this.changed();
    return dropped;
  }

  /** The worst priority among droppable waiting jobs, or `null` when none. */
  worstDroppable(): number | null {
    let worst: number | null = null;
    for (const job of this.waiting) {
      if (job.droppable && (worst === null || job.priority > worst)) worst = job.priority;
    }
    return worst;
  }

  /** Drop the worst-ranked droppable waiting job. @returns whether one went. */
  evictWorst(): boolean {
    let at = -1;
    for (let i = 0; i < this.waiting.length; i += 1) {
      const job = this.waiting[i]!;
      if (job.droppable && (at < 0 || job.priority >= this.waiting[at]!.priority)) at = i;
    }
    if (at < 0) return false;
    const [job] = this.waiting.splice(at, 1);
    job!.drop();
    this.changed();
    return true;
  }

  private pump(): void {
    while (this.running < this.limit && this.waiting.length > 0) {
      let best = 0;
      for (let i = 1; i < this.waiting.length; i += 1) {
        // Strict `<` keeps arrival order among equals.
        if (this.waiting[i]!.priority < this.waiting[best]!.priority) best = i;
      }
      const [job] = this.waiting.splice(best, 1);
      job!.start();
    }
    this.changed();
  }

  private changed(): void {
    this.onChange?.({ active: this.running, waiting: this.waiting.length, dropped: this.dropped });
  }
}

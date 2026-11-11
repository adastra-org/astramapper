import { describe, expect, test } from "bun:test";
import { InstanceSlots, MIN_CAPACITY } from "./instanceSlots";

describe("InstanceSlots", () => {
  test("hands out dense slots from zero", () => {
    const slots = new InstanceSlots();
    expect(slots.add(10)).toBe(0);
    expect(slots.add(11)).toBe(1);
    expect(slots.count).toBe(2);
    expect(slots.ownerAt(1)).toBe(11);
    expect(slots.ownerAt(2)).toBeUndefined();
    expect(slots.ownerAt(-1)).toBeUndefined();
  });

  test("an owner holds one slot", () => {
    const slots = new InstanceSlots();
    slots.add(1);
    expect(slots.add(1)).toBeUndefined();
    expect(slots.count).toBe(1);
  });

  test("removing the last slot needs no move", () => {
    const slots = new InstanceSlots();
    slots.add(1);
    slots.add(2);
    expect(slots.remove(2)).toEqual({ slot: 1 });
    expect(slots.count).toBe(1);
    expect(slots.has(2)).toBe(false);
  });

  test("removing from the middle swaps the last one into the hole", () => {
    const slots = new InstanceSlots();
    for (const owner of [1, 2, 3, 4]) slots.add(owner);
    const result = slots.remove(2);
    expect(result).toEqual({ slot: 1, moved: { from: 3, to: 1 } });
    expect(slots.count).toBe(3);
    expect(slots.slotOf(4)).toBe(1);
    expect(slots.ownerAt(1)).toBe(4);
    expect([...slots.owners()]).toEqual([1, 4, 3]);
  });

  test("removing the only owner empties the table", () => {
    const slots = new InstanceSlots();
    slots.add(1);
    expect(slots.remove(1)).toEqual({ slot: 0 });
    expect(slots.count).toBe(0);
    expect(slots.ownerAt(0)).toBeUndefined();
  });

  test("an unknown owner removes to null", () => {
    expect(new InstanceSlots().remove(9)).toBeNull();
  });

  test("a full table refuses until grown, and growing keeps every slot", () => {
    const slots = new InstanceSlots();
    for (let i = 0; i < MIN_CAPACITY; i += 1) slots.add(i);
    expect(slots.full).toBe(true);
    expect(slots.add(99)).toBeUndefined();
    expect(slots.grow()).toBe(MIN_CAPACITY * 2);
    expect(slots.add(99)).toBe(MIN_CAPACITY);
    for (let i = 0; i < MIN_CAPACITY; i += 1) expect(slots.slotOf(i)).toBe(i);
  });

  test("shrinks only when under a quarter full, never below the minimum", () => {
    const slots = new InstanceSlots(64);
    for (let i = 0; i < 17; i += 1) slots.add(i);
    expect(slots.shrinkTarget()).toBeUndefined();
    for (let i = 16; i >= 16 - 1; i -= 1) slots.remove(i);
    // 15 of 64 is under a quarter.
    expect(slots.shrinkTarget()).toBe(32);
    slots.resize(32);
    expect(slots.capacity).toBe(32);
    expect(slots.slotOf(14)).toBe(14);
    expect(new InstanceSlots().shrinkTarget()).toBeUndefined();
  });

  test("resize never cuts live slots", () => {
    const slots = new InstanceSlots(16);
    for (let i = 0; i < 10; i += 1) slots.add(i);
    expect(slots.resize(2)).toBe(10);
  });

  test("stays consistent over a churn of adds and removes", () => {
    const slots = new InstanceSlots();
    const live = new Set<number>();
    let next = 0;
    let seed = 7;
    const rand = (): number => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed / 0x7fffffff;
    };
    for (let step = 0; step < 2000; step += 1) {
      if (live.size === 0 || rand() < 0.55) {
        if (slots.full) slots.grow();
        slots.add(next);
        live.add(next);
        next += 1;
      } else {
        const pick = [...live][Math.floor(rand() * live.size)]!;
        slots.remove(pick);
        live.delete(pick);
        const target = slots.shrinkTarget();
        if (target !== undefined) slots.resize(target);
      }
      expect(slots.count).toBe(live.size);
    }
    for (const owner of live) expect(slots.ownerAt(slots.slotOf(owner)!)).toBe(owner);
  });
});

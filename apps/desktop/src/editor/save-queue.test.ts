import { expect, it, vi } from 'vitest';
import { createSaveQueue } from './save-queue';

it('serializes updates that arrive while a previous revision is saving', async () => {
  const writes: [string, number][] = [];
  const pending = Promise.withResolvers<{ revision: number }>();
  const started = Promise.withResolvers<void>();
  const queue = createSaveQueue(1, async (content: string, revision) => {
    writes.push([content, revision]);
    if (writes.length === 1) started.resolve();
    return writes.length === 1 ? pending.promise : { revision: revision + 1 };
  }, () => {}, 60_000);
  queue.update('第一稿');
  const save = queue.flush();
  await started.promise;
  expect(queue.discard()).toBe(false);
  queue.update('最新稿');
  pending.resolve({ revision: 2 });
  await save;
  expect(writes).toEqual([['第一稿', 1], ['最新稿', 2]]);
  expect(queue.dirty).toBe(false);
});

it('preserves the latest draft for a manual retry after a failed save', async () => {
  const writes: [string, number][] = [];
  const queue = createSaveQueue(5, async (content: string, revision) => {
    writes.push([content, revision]);
    if (writes.length === 1) throw new Error('disk full');
    return { revision: revision + 1 };
  }, () => {}, 60_000);
  queue.update('草稿');
  await expect(queue.flush()).rejects.toThrow('disk full');
  expect(queue.dirty).toBe(true);
  queue.update('继续编辑');
  await queue.flush();
  expect(writes).toEqual([['草稿', 5], ['继续编辑', 5]]);
  expect(queue.dirty).toBe(false);
});

it('stops scheduled and late edits after discarding an unsaved draft', async () => {
  vi.useFakeTimers();
  try {
    const write = vi.fn(async (_content: string, _revision: number): Promise<{ revision: number }> => {
      throw new Error('page removed');
    });
    const queue = createSaveQueue(5, write, () => {});
    queue.update('未能保存的草稿');
    await expect(queue.flush()).rejects.toThrow('page removed');
    queue.update('关闭前的最后一稿');

    expect(queue.discard()).toBe(true);
    queue.update('迟到的输入事件');
    await vi.runAllTimersAsync();
    await expect(queue.flush()).resolves.toBeUndefined();
    expect(queue.dirty).toBe(false);
    expect(write.mock.calls).toEqual([['未能保存的草稿', 5]]);
  } finally {
    vi.useRealTimers();
  }
});

it('schedules a draft entered at the completion of a previous save', async () => {
  const first = Promise.withResolvers<{ revision: number }>();
  const started = Promise.withResolvers<void>();
  const second = Promise.withResolvers<void>();
  const writes: [string, number][] = [];
  const queue = createSaveQueue(1, (content: string, revision) => {
    writes.push([content, revision]);
    if (writes.length === 1) {
      started.resolve();
      return first.promise;
    }
    second.resolve();
    return Promise.resolve({ revision: revision + 1 });
  }, () => {}, 0);
  queue.update('第一稿');
  const initial = queue.flush();
  await started.promise;
  first.resolve({ revision: 2 });
  queueMicrotask(() => queue.update('保存结束时的新稿'));
  await initial;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([second.promise, new Promise<void>((resolve) => { timeout = setTimeout(resolve, 80); })]);
    expect(writes).toEqual([['第一稿', 1], ['保存结束时的新稿', 2]]);
    expect(queue.dirty).toBe(false);
  } finally {
    clearTimeout(timeout);
  }
});

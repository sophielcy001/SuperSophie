// SPDX-License-Identifier: AGPL-3.0-or-later

export type SaveStatus = 'pending' | 'saving' | 'saved' | 'error';

export interface SaveQueue<T, R> {
  update(draft: T): void;
  flush(): Promise<R | undefined>;
  discard(): boolean;
  get revision(): number;
  get dirty(): boolean;
}

export function createSaveQueue<T, R extends { revision: number }>(
  revision: number,
  save: (draft: T, revision: number) => Promise<R>,
  status: (state: SaveStatus, error?: unknown) => void,
  delay = 550,
): SaveQueue<T, R> {
  let latest: { draft: T } | undefined;
  let lastResult: R | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let running: Promise<R | undefined> | undefined;
  let discarded = false;

  function flush() {
    clearTimeout(timer);
    if (running) return running;
    if (!latest) return Promise.resolve(lastResult);
    running = Promise.resolve().then(async () => {
      try {
        while (latest) {
          const current = latest;
          status('saving');
          const result = await save(current.draft, revision);
          revision = result.revision;
          lastResult = result;
          if (current === latest) latest = undefined;
        }
        running = undefined;
        status('saved');
        return lastResult;
      } catch (error) {
        running = undefined;
        status('error', error);
        throw error;
      }
    });
    return running;
  }

  return {
    update(draft) {
      if (discarded) return;
      latest = { draft };
      if (running) return;
      status('pending');
      clearTimeout(timer);
      timer = setTimeout(() => { void flush().catch(() => {}); }, delay);
    },
    flush,
    discard() {
      if (running) return false;
      clearTimeout(timer);
      latest = undefined;
      discarded = true;
      return true;
    },
    get revision() { return revision; },
    get dirty() { return latest !== undefined; },
  };
}

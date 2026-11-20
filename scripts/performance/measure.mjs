// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { arch, cpus, release, totalmem } from 'node:os';
import { createInterface } from 'node:readline';
import { resolve } from 'node:path';

export const elapsed = (start) => performance.now() - start;

export function environment() {
  return { platform: process.platform, release: release(), architecture: arch(), cpu: cpus()[0]?.model,
    bytesRam: totalmem(), node: process.version };
}

export async function fileSha256(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

export function metrics(samples) {
  const values = samples.toSorted((a, b) => a - b);
  const percentile = (rate) => values[Math.ceil(values.length * rate) - 1];
  return {
    n: values.length, p50Ms: percentile(.5) ?? null, p95Ms: percentile(.95) ?? null,
    maxMs: values.at(-1) ?? null, samplesMs: samples,
  };
}

export function coreAdapter(data) {
  const example = process.platform === 'win32' ? 'm5_adapter.exe' : 'm5_adapter';
  const executable = process.env.ZHIYE_PERF_ADAPTER ?? resolve('target/debug/examples', example);
  const child = spawn(executable, [data], { stdio: ['pipe', 'pipe', 'inherit'] });
  let failure;
  const exited = new Promise((resolve) => child.once('close', resolve));
  const replies = createInterface({ input: child.stdout });
  const pending = new Map();
  let request = 0;
  let stopped = false;
  function finish(error) {
    stopped = true;
    for (const { reject } of pending.values()) reject(error);
    pending.clear();
  }
  child.on('error', (error) => { failure = error; finish(error); });
  child.on('exit', () => finish(new Error('Core adapter exited')));
  replies.on('line', (line) => {
    const response = JSON.parse(line);
    const entry = pending.get(response.id);
    assert.ok(entry, 'Reply matches an outstanding Core request');
    pending.delete(response.id);
    if (response.error) entry.reject(response.error);
    else entry.resolve(response.result);
  });
  return {
    call(cmd, args = {}) {
      if (stopped) return Promise.reject(new Error('Core adapter is closed'));
      return new Promise((resolve, reject) => {
        const id = ++request;
        pending.set(id, { resolve, reject });
        child.stdin.write(JSON.stringify({ id, cmd, args }) + '\n');
      });
    },
    async close() {
      child.stdin.end();
      const code = await exited;
      replies.close();
      if (failure) throw failure;
      assert.equal(code, 0, 'Core adapter exit status');
    },
  };
}

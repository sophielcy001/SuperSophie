// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';

const entries = process.argv.slice(2);
assert.ok(entries.length, 'Usage: node scripts/dependency-inventory.mjs <target>=<cargo-metadata.json> [...]');
const [npmLockBytes, cargoLockBytes] = await Promise.all(['package-lock.json', 'Cargo.lock']
  .map((path) => readFile(new URL(`../${path}`, import.meta.url))));
const npmLock = JSON.parse(npmLockBytes);
const lockHash = (bytes) => createHash('sha256').update(bytes).digest('hex');

const npm = Object.entries(npmLock.packages).flatMap(([path, entry]) => {
  const segments = path.split('node_modules/');
  if (entry.link || segments.length === 1) return [];
  const name = entry.name ?? segments.at(-1);
  assert.ok(entry.version && entry.license && entry.resolved && entry.integrity, `${path} requires locked metadata`);
  return [{ name, version: entry.version, path, license: entry.license, source: entry.resolved,
    integrity: entry.integrity, use: entry.dev ? 'development' : entry.devOptional ? 'development-or-optional' : 'production',
    optional: Boolean(entry.optional || entry.devOptional), os: entry.os, cpu: entry.cpu, libc: entry.libc }];
});

function cargoPackages(metadata) {
  const packages = new Map(metadata.packages.map((entry) => [entry.id, entry]));
  const thirdParty = new Map(metadata.packages.filter(({ source }) => source).map((entry) => {
    assert.ok(entry.license || entry.license_file, `${entry.name}@${entry.version} requires a license declaration`);
    return [entry.id, { name: entry.name, version: entry.version, license: entry.license,
      licenseFile: entry.license_file ?? undefined, source: entry.source, usedBy: [] }];
  }));
  const normalizeAlias = (name) => name.replaceAll('-', '_');
  for (const parent of metadata.resolve.nodes) {
    const origin = packages.get(parent.id);
    for (const resolved of parent.deps) {
      const output = thirdParty.get(resolved.pkg);
      if (!output) continue;
      for (const { kind, target } of resolved.dep_kinds) {
        const declarations = origin.dependencies.filter((dependency) => dependency.name === output.name
          && dependency.kind === kind && dependency.target === target);
        const aliased = declarations.filter((dependency) => normalizeAlias(dependency.rename ?? dependency.name) === resolved.name);
        const flags = new Set((aliased.length ? aliased : declarations).map(({ optional }) => optional));
        assert.equal(flags.size, 1, `Cannot identify manifest dependency ${origin.name} -> ${output.name}`);
        output.usedBy.push({ name: origin.name, version: origin.version, kind: kind ?? 'normal',
          optional: [...flags][0], target: target ?? undefined });
      }
    }
  }
  return [...thirdParty.values()].sort((left, right) => left.name.localeCompare(right.name, 'en')
    || left.version.localeCompare(right.version, 'en'));
}

const cargo = {};
for (const entry of entries) {
  const separator = entry.indexOf('=');
  const target = entry.slice(0, separator);
  assert.ok(separator > 0 && /^[a-z0-9_-]+$/.test(target) && !Object.hasOwn(cargo, target), `Invalid target ${entry}`);
  const metadata = JSON.parse(await readFile(entry.slice(separator + 1), 'utf8'));
  const workspace = metadata.packages.filter(({ id }) => metadata.workspace_members.includes(id));
  assert.deepEqual(workspace.map(({ name }) => name).sort(), ['zhiye-core', 'zhiye-desktop'], target);
  assert.ok(workspace.every(({ version }) => version === npmLock.version), `${target} metadata must use the current version`);
  cargo[target] = cargoPackages(metadata);
}

process.stdout.write(`${JSON.stringify({ version: npmLock.version,
  locks: { npm: lockHash(npmLockBytes), cargo: lockHash(cargoLockBytes) }, npm, cargo }, null, 2)}\n`);

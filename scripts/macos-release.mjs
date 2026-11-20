// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  constants, copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync,
  readFileSync, rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

assert.equal(process.platform, 'darwin', '请在 Mac 上构建 macOS 发布包。');
assert.ok(process.argv.length <= 3, 'usage: npm run release:mac -- [output-directory]');
const root = fileURLToPath(new URL('../', import.meta.url));
const output = resolve(root, process.argv[2] ?? 'artifacts');
const { version } = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
const config = JSON.parse(readFileSync(join(root, 'apps', 'desktop', 'src-tauri', 'tauri.macos.conf.json'), 'utf8'));
const hostInfo = execFileSync('rustc', ['-vV'], { encoding: 'utf8' });
const target = /^host: (.+)$/m.exec(hostInfo)?.[1];
const architectures = { 'aarch64-apple-darwin': 'arm64', 'x86_64-apple-darwin': 'x64' };
assert.ok(Object.hasOwn(architectures, target), '需要原生 macOS Rust 工具链。');
const name = `Zhiye-${version}-macos-${architectures[target]}.dmg`;
const artifact = join(output, name);
assert.ok(!existsSync(artifact) && !existsSync(artifact + '.sha256'), '请使用尚不存在的输出文件或另一输出目录。');

const temporary = mkdtempSync(join(tmpdir(), 'zhiye-macos-release-'));
const cargoTarget = resolve(root, process.env.CARGO_TARGET_DIR ?? 'target');
const stagingTarget = join(temporary, 'target');
const environment = { ...process.env, MACOSX_DEPLOYMENT_TARGET: config.bundle.macOS.minimumSystemVersion };

function run(command, args, cwd = root, env = environment) {
  const result = spawnSync(command, args, { cwd, env, stdio: 'inherit' });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${command} 执行失败${result.signal ? ` (${result.signal})` : ''}`);
}

try {
  run('npm', ['run', 'build:ui']);
  run('cargo', ['build', '--locked', '--release', '--target', target,
    '-p', 'zhiye-desktop', '--features', 'custom-protocol'],
    root, { ...environment, CARGO_TARGET_DIR: cargoTarget });

  const release = join(stagingTarget, target, 'release');
  mkdirSync(release, { recursive: true });
  copyFileSync(join(cargoTarget, target, 'release', 'zhiye-desktop'), join(release, 'zhiye-desktop'));
  run(process.execPath, [join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js'),
    'bundle', '--target', target, '--bundles', 'app', '--ci'],
    join(root, 'apps', 'desktop'),
    { ...environment, CARGO_TARGET_DIR: stagingTarget, APPLE_SIGNING_IDENTITY: '-' });

  const application = join(release, 'bundle', 'macos', '知页.app');
  run('/usr/bin/codesign', ['--verify', '--deep', '--strict', application]);
  const observedVersion = execFileSync('/usr/bin/plutil', ['-extract', 'CFBundleShortVersionString',
    'raw', '-o', '-', join(application, 'Contents', 'Info.plist')], { encoding: 'utf8' }).trim();
  assert.equal(observedVersion, version, '应用包版本与源码不一致。');

  const imageRoot = join(temporary, 'install');
  mkdirSync(imageRoot);
  const imageApplication = join(imageRoot, '知页.app');
  cpSync(application, imageApplication, { recursive: true, verbatimSymlinks: true });
  run('/usr/bin/codesign', ['--verify', '--deep', '--strict', imageApplication]);
  symlinkSync('/Applications', join(imageRoot, 'Applications'));
  const image = join(temporary, name);
  run('/usr/bin/hdiutil', ['create', '-quiet', '-volname', `知页 ${version}`, '-srcfolder', imageRoot,
    '-fs', 'HFS+', '-format', 'UDZO', '-o', image]);
  run('/usr/bin/hdiutil', ['verify', '-quiet', image]);

  mkdirSync(output, { recursive: true });
  copyFileSync(image, artifact, constants.COPYFILE_EXCL);
  const sha256 = createHash('sha256').update(readFileSync(artifact)).digest('hex');
  writeFileSync(artifact + '.sha256', `${sha256}  ${name}\n`, { flag: 'wx' });
  console.log(`已生成 ${artifact}`);
  console.log(`SHA-256 ${sha256}`);
} finally {
  rmSync(temporary, { recursive: true, force: true });
}

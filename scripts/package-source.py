# SPDX-License-Identifier: AGPL-3.0-or-later

"""Create a source archive from the project's build inputs and synthetic fixtures."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo


ROOT = Path(__file__).resolve().parents[1]
FILES = (
    '.gitignore', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
    'package.json', 'package-lock.json', 'README.md', 'CONTRIBUTING.md',
    'LICENSE', 'THIRD_PARTY_NOTICES.md', '个人资源管理系统_完整开发Plan_v2.0.md',
    'apps/desktop/index.html', 'apps/desktop/package.json', 'apps/desktop/tsconfig.json',
    'apps/desktop/vite.config.ts', 'apps/desktop/src-tauri/Cargo.toml',
    'apps/desktop/src-tauri/build.rs', 'apps/desktop/src-tauri/tauri.conf.json',
    'apps/desktop/src-tauri/tauri.windows.conf.json', 'apps/desktop/src-tauri/tauri.macos.conf.json',
    'apps/desktop/src-tauri/windows/installer-hooks.nsh', 'apps/desktop/src-tauri/windows/installer.nsi',
    'crates/core/Cargo.toml', 'crates/sync-protocol/Cargo.toml', 'crates/sync-server/Cargo.toml',
    'deploy/sync/Dockerfile', 'deploy/sync/Dockerfile.dockerignore', 'deploy/sync/compose.yaml',
    'deploy/sync/Caddyfile', 'deploy/sync/.env.example', 'deploy/sync/init-secrets.sh',
    'packages/mcp/package.json', 'packages/mcp/tsconfig.json',
    'packages/sdk-ts/package.json', 'packages/sdk-ts/tsconfig.json',
    '.verify/m2-browser/verify.mjs', '.verify/m2-browser/driver/Cargo.toml',
    '.verify/m2-browser/driver/Cargo.lock',
)
TREES = (
    '.github', 'docs', 'licenses', 'examples', 'scripts', 'apps/desktop/src',
    'apps/desktop/src-tauri/src', 'apps/desktop/src-tauri/capabilities',
    'apps/desktop/src-tauri/icons', 'apps/desktop/src-tauri/examples',
    'crates/core/src', 'crates/core/examples', 'crates/core/tests',
    'crates/sync-protocol/src', 'crates/sync-server/src', 'crates/sync-server/tests',
    'packages/mcp/src', 'packages/mcp/test', 'packages/sdk-ts/src', 'packages/sdk-ts/test',
    '.verify/m2-browser/driver/src', '.verify/migration/fixtures',
    '.verify/pdf-index/fixtures', '.verify/rich-blocks/fixtures',
)
SUFFIXES = {
    '.canvas', '.css', '.html', '.ico', '.js', '.json', '.md', '.mjs', '.pdf',
    '.png', '.ps1', '.py', '.rs', '.sql', '.toml', '.ts', '.tsx', '.txt', '.yaml', '.zip',
}
EXCLUDED_DIRS = {'dist', 'gen', 'target', 'node_modules', '__pycache__'}
EXCLUDED_TREES = {Path('docs/验收记录')}


def require_regular(relative, *, directory=False):
    path = ROOT / relative
    entry = path.lstat()
    kind = stat.S_ISDIR if directory else stat.S_ISREG
    if not kind(entry.st_mode) or getattr(entry, 'st_file_attributes', 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        label = 'directory' if directory else 'file'
        raise ValueError(f'Expected a regular project {label}: {relative.as_posix()}')
    return path


def check_ancestors(relative):
    for folder in reversed(relative.parents):
        if folder.parts:
            require_regular(folder, directory=True)


def raise_walk_error(error):
    raise error


def inputs():
    relatives = {Path(name) for name in FILES}
    for tree in TREES:
        relative_tree = Path(tree)
        check_ancestors(relative_tree)
        directory = require_regular(relative_tree, directory=True)
        for folder, parents, files in os.walk(directory, onerror=raise_walk_error):
            relative_folder = Path(folder).relative_to(ROOT)
            parents[:] = [name for name in parents
                          if name not in EXCLUDED_DIRS and (relative_folder / name) not in EXCLUDED_TREES]
            for name in parents:
                require_regular(relative_folder / name, directory=True)
            for name in files:
                if not name.startswith('._') and Path(name).suffix in SUFFIXES:
                    relatives.add(relative_folder / name)

    seen = set()
    for relative in sorted(relatives, key=Path.as_posix):
        check_ancestors(relative)
        path = require_regular(relative)
        name = relative.as_posix()
        if name.casefold() in seen:
            raise ValueError(f'Case-insensitive archive name collision: {name}')
        seen.add(name.casefold())
        yield name, path.read_bytes()


def write_entry(archive, name, content):
    entry = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    entry.create_system = 3
    entry.external_attr = 0o100644 << 16
    archive.writestr(entry, content, compress_type=ZIP_DEFLATED, compresslevel=9)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    files = dict(inputs())
    version = json.loads(files['package.json'])['version']
    basename = f'Zhiye-{version}-source'
    manifest = {name: hashlib.sha256(content).hexdigest() for name, content in files.items()}
    inventory = json.dumps({'version': version, 'sha256': manifest}, ensure_ascii=False, indent=2) + '\n'
    with args.output.open('x+b') as output:
        try:
            with ZipFile(output, 'w') as archive:
                for name, content in files.items():
                    write_entry(archive, f'{basename}/{name}', content)
                write_entry(archive, f'{basename}/SOURCE-MANIFEST.json', inventory.encode('utf-8'))
            output.flush()
            with ZipFile(output) as archive:
                invalid = archive.testzip()
                if invalid is not None:
                    raise ValueError(f'Corrupted ZIP member: {invalid}')
            output.seek(0)
            digest = hashlib.sha256()
            for chunk in iter(lambda: output.read(1024 * 1024), b''):
                digest.update(chunk)
        except BaseException:
            output.close()
            args.output.unlink()
            raise
    print(f'{digest.hexdigest()}  {args.output.name}')


if __name__ == '__main__':
    main()

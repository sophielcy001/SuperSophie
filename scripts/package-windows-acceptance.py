#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later

"""Assemble a portable Windows acceptance ZIP from verified release inputs and caches."""

import argparse
import hashlib
import json
import posixpath
import re
from pathlib import Path, PurePosixPath
from urllib.parse import urlsplit
from zipfile import ZIP_STORED, ZipFile, ZipInfo


PROJECT = Path(__file__).resolve().parents[1]
LINK = re.compile(r'(?<!!)\[([^\[\]\n]+)\]\(([^)\n]+)\)')
FENCE = re.compile(r'(^```[^\n]*\n.*?^```[ \t]*$)', re.M | re.S)
FIXTURES = {
    '.verify/a06-current/a06-synthetic-inputs.zip': 'fixtures/A06/a06-synthetic-inputs.zip',
    '.verify/a15-current/a15-synthetic-inputs.zip': 'fixtures/A15/a15-synthetic-inputs.zip',
}
REGISTRY = 'index.crates.io-1949cf8c6b5b557f'
WINDOWS_NAMES = {'con', 'nul', 'prn', 'aux'} | {f'{port}{i}' for port in ('com', 'lpt') for i in range(1, 10)}
# Unmodified upstream assets for the Tauri CLI locked in package-lock.json.
NSIS_TOOLS = {
    'nsis-3.13.zip': 'ba63dffc4410ee89193e1cb5a41989991bd77c61068da17e3156d136b7b0b3d8',
    'nsis_tauri_utils.dll': '5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709',
    'nsis-3.13-src.tar.bz2': 'a8ffe024602d46b6d766f9e1ce30c324ad2a24daeacd3efc2642d436a0c157ac',
    'tauri-nsis-plugin-source.zip': '4f42fb387ea0e39a0ce66993d32382fdfa637a0bd01d5ea006a8e69df449bde9',
}


def digest(stream):
    sha = hashlib.sha256()
    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
        sha.update(chunk)
    return sha.hexdigest()


def file_digest(path):
    with path.open('rb') as stream:
        return digest(stream)


def verified_file(path):
    if not path.is_file() or path.resolve() != path:
        raise ValueError(f'Expected a regular file: {path}')
    expected = path.with_name(path.name + '.sha256').read_text().split()[0]
    if file_digest(path) != expected:
        raise ValueError(f'SHA-256 mismatch: {path}')
    return expected


def windows_safe(name):
    path = PurePosixPath(name)
    return bool(path.parts) and not path.is_absolute() and all(
        part not in ('', '.', '..') and part[-1] not in '. '
        and not re.search(r'[<>:"\\|?*\x00-\x1f]', part)
        and part.split('.')[0].casefold() not in WINDOWS_NAMES
        for part in path.parts
    )


def cache_inputs(root):
    trees = ('npm/_cacache/content-v2', 'npm/_cacache/index-v5',
             f'cargo/registry/cache/{REGISTRY}', f'cargo/registry/index/{REGISTRY}/.cache')
    for tree in trees:
        folder = root / tree
        if not folder.is_dir():
            raise FileNotFoundError(folder)
        for file in sorted(folder.rglob('*')):
            if file.is_file():
                yield file
    yield root / f'cargo/registry/index/{REGISTRY}/config.json'
    yield root / 'LOCKS.json'


def portable_markdown(text, source, destination, links, source_members, source_zip):
    def resolve(match):
        label, address = match.groups()
        path, marker, anchor = address.partition('#')
        if not path or urlsplit(path).scheme:
            return match[0]
        target = posixpath.normpath(posixpath.join(posixpath.dirname(source), path))
        if target in links:
            relative = posixpath.relpath(links[target], posixpath.dirname(destination) or '.')
            return f'[{label}]({relative}{marker}{anchor})'
        if target in source_members or any(name.startswith(target + '/') for name in source_members):
            relative = posixpath.relpath(source_zip, posixpath.dirname(destination) or '.')
            return f'[{label}]({relative})（源码内 `{target}`）'
        return f'{label}（项目记录）'

    return ''.join(part if i % 2 else LINK.sub(resolve, part) for i, part in enumerate(FENCE.split(text)))


def write_entry(archive, name, content):
    entry = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    entry.compress_type = ZIP_STORED
    entry.external_attr = 0o100644 << 16
    if isinstance(content, bytes):
        archive.writestr(entry, content)
        return hashlib.sha256(content).hexdigest()
    sha = hashlib.sha256()
    with content.open('rb') as original, archive.open(entry, 'w') as dest:
        for chunk in iter(lambda: original.read(1024 * 1024), b''):
            sha.update(chunk)
            dest.write(chunk)
    return sha.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('dependencies', type=Path, help='prepared Windows x64 npm and Cargo cache directory')
    parser.add_argument('output', type=Path)
    parser.add_argument('--nsis-tools', type=Path, help='pinned NSIS and plug-in binary and source archives')
    args = parser.parse_args()
    dependencies = args.dependencies.resolve(strict=True)
    version = json.loads((PROJECT / 'package.json').read_text())['version']
    source_zip = f'artifacts/Zhiye-{version}-source.zip'
    archive_prefix = f'Zhiye-{version}-source/'
    base = f'Zhiye-{version}-windows-acceptance'
    files = {}
    links = {}
    verified = {}

    if args.nsis_tools:
        tools = args.nsis_tools.resolve(strict=True)
        for name, expected in NSIS_TOOLS.items():
            path = tools / name
            if not path.is_file() or path.resolve() != path or file_digest(path) != expected:
                raise ValueError(f'Missing or invalid NSIS tool input: {path}')
            target = f'dependencies/nsis/{name}'
            files[target] = path
            verified[target] = expected

    def add(source, dest=None):
        path = PROJECT / source
        target = dest or source
        if target in files or not path.is_file() or path.resolve() != path:
            raise ValueError(f'Cannot include {source} as {target}')
        files[target] = path
        links[source] = target

    for name in (source_zip, f'artifacts/Zhiye-{version}-win-x64-setup.exe',
                 f'artifacts/Zhiye-{version}-win-x64-offline-setup.exe', 'artifacts/Zhiye-0.31.0-win-x64-setup.exe',
                 'artifacts/Zhiye-0.31.0-A16-synthetic.zhiye-recovery.zip', *FIXTURES):
        output = FIXTURES.get(name, name)
        verified[output] = verified_file(PROJECT / name)
        add(name, output)
        add(name + '.sha256', output + '.sha256')

    with ZipFile(PROJECT / source_zip) as archive:
        manifest = json.loads(archive.read(archive_prefix + 'SOURCE-MANIFEST.json'))
        assert manifest['version'] == version
        source_members = set(manifest['sha256'])
    cache_metadata = json.loads((dependencies / 'LOCKS.json').read_text())
    assert cache_metadata['version'] == version and cache_metadata['target'] == 'x86_64-pc-windows-msvc'
    assert cache_metadata['sha256'] == {name: manifest['sha256'][name] for name in ('Cargo.lock', 'package-lock.json')}

    for name in ('README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md', '个人资源管理系统_完整开发Plan_v2.0.md'):
        add(name)
    for path in sorted((PROJECT / 'docs').iterdir()):
        if path.is_file() and path.suffix in ('.md', '.yaml'):
            name = path.relative_to(PROJECT).as_posix()
            add(name, '开始验收.md' if name == 'docs/Windows验收包.md' else None)
    for directory in ('docs/contracts', 'licenses'):
        for path in sorted((PROJECT / directory).iterdir()):
            if path.is_file():
                add(path.relative_to(PROJECT).as_posix())
    links[f'artifacts/{base}.zip'] = '开始验收.md'

    for name, dest in links.items():
        if not name.endswith('.md'):
            continue
        original = files[dest].read_text(encoding='utf-8')
        files[dest] = portable_markdown(original, name, dest, links, source_members, source_zip).encode('utf-8')

    for path in cache_inputs(dependencies):
        if not path.is_file() or path.resolve() != path:
            raise ValueError(f'Invalid cache input: {path}')
        files['dependencies/' + path.relative_to(dependencies).as_posix()] = path

    if not all(windows_safe(name) for name in files) or len({name.casefold() for name in files}) != len(files):
        raise ValueError('Package paths must be unique on Windows and use valid Windows names')

    with args.output.open('xb') as output:
        try:
            with ZipFile(output, 'w') as archive:
                hashes = {name: write_entry(archive, f'{base}/{name}', files[name]) for name in sorted(files)}
                for name, expected in verified.items():
                    if hashes[name] != expected:
                        raise ValueError(f'Input changed during packaging: {name}')
                sums = ''.join(f'{value}  {name}\n' for name, value in hashes.items())
                write_entry(archive, f'{base}/SHA256SUMS.txt', sums.encode('utf-8'))
        except BaseException:
            output.close()
            args.output.unlink()
            raise

    with ZipFile(args.output) as archive:
        if archive.testzip():
            raise ValueError('Invalid ZIP member')
    sha = file_digest(args.output)
    sidecar = args.output.with_name(args.output.name + '.sha256')
    with sidecar.open('x', encoding='utf-8') as sums:
        sums.write(f'{sha}  {args.output.name}\n')
    print(f'{sha}  {args.output.name}')
    print(f'{len(hashes)} files verified')


if __name__ == '__main__':
    main()

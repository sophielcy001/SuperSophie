#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later

"""Replay the Windows A18 audit against a returned Run-A18.ps1 evidence directory."""

import argparse
from datetime import datetime
from hashlib import file_digest, sha256
import json
from pathlib import Path, PureWindowsPath
import re
import subprocess
import sys
from tempfile import TemporaryDirectory, gettempdir
from zipfile import BadZipFile, ZipFile


KIT_FILES = (
    'audit-run.mjs', 'reference-device.mjs', 'packages.json', 'fixture-inputs.json',
    'scripts/performance/measure.mjs', 'scripts/performance/windows-results.mjs', 'tools/node.exe',
)
REPORT_FILES = (
    'baseline-core.json', 'baseline-release-webview2.json', 'candidate-performance.json',
    'cleanup.json', 'device.json', 'eligibility.json', 'fixture-identity.json', 'http.json',
    'install.json', 'installed-startup.json', 'pressure-core.json', 'pressure-release-webview2.json',
    'scratch.path', 'summary.json',
)
PHASES = ('fixture-baseline', 'fixture-pressure', 'startup', 'release', 'http')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def declared_zip_sha256(path, label):
    expected, name = path.with_suffix('.zip.sha256').read_text(encoding='utf-8-sig').strip().split('  ', 1)
    require(name == path.name and re.fullmatch(r'[a-f0-9]{64}', expected), f'invalid {label} checksum file')
    return expected


def handoff_batch(path):
    archive_sha = declared_zip_sha256(path, 'handoff')
    with path.open('rb') as file:
        require(file_digest(file, 'sha256').hexdigest() == archive_sha, 'handoff SHA-256 mismatch')
        with ZipFile(file) as archive:
            members = archive.namelist()
            ledgers = [name for name in members if name.endswith('/SHA256SUMS.txt')]
            require(len(ledgers) == 1 and len(members) == len(set(members)), 'invalid handoff member inventory')
            ledger = ledgers[0]
            match = re.fullmatch(r'Zhiye-([A-Za-z0-9][A-Za-z0-9._-]*)-windows-acceptance/SHA256SUMS\.txt', ledger)
            require(match is not None, 'invalid Windows handoff directory')
            version = match[1]
            prefix = ledger.removesuffix('SHA256SUMS.txt')
            lines = archive.read(ledger).decode('utf-8').splitlines()
            checksums = {name: digest for digest, name in (line.split('  ', 1) for line in lines)}
            require(len(checksums) == len(lines), 'duplicate entry in handoff checksum ledger')
            base = 'artifacts/Zhiye-' + version
            payloads = {
                'kitSha256': base + '-windows-a18.zip',
                'sourceSha256': base + '-source.zip',
                'installerSha256': base + '-win-x64-setup.exe',
            }
            matched = {}
            for label, name in payloads.items():
                message = 'handoff does not contain a checked A18 kit' if label == 'kitSha256' else (
                    f'handoff does not contain a checked release file: {name}'
                )
                require(name in checksums and prefix + name in members, message)
                with archive.open(prefix + name) as content:
                    matched[label] = file_digest(content, 'sha256').hexdigest()
                require(matched[label] == checksums[name], f'handoff member SHA-256 mismatch: {name}')
    return {'handoffArchiveSha256': archive_sha, 'version': version, **matched}


def load_kit(path, target, batch=None):
    expected = declared_zip_sha256(path, 'A18 kit')
    if batch is not None:
        require(expected == batch['kitSha256'], 'A18 kit does not match the handoff')
    with path.open('rb') as file:
        require(file_digest(file, 'sha256').hexdigest() == expected, 'A18 kit SHA-256 mismatch')
        with ZipFile(file) as archive:
            prefix = path.stem + '/'
            manifest = json.loads(archive.read(prefix + 'SHA256SUMS.json'))['files']
            require(len(archive.namelist()) == len(manifest) + 1 and set(archive.namelist()) ==
                    {prefix + name for name in (*manifest, 'SHA256SUMS.json')}, 'invalid A18 kit inventory')
            for name in KIT_FILES:
                content = archive.read(prefix + name)
                require(sha256(content).hexdigest() == manifest[name], f'A18 kit member mismatch: {name}')
                dest = target / name
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_bytes(content)
    if batch is not None:
        standard = json.loads((target / 'packages.json').read_bytes())['standard']
        require((standard['productVersion'], standard['sourceSha256'], standard['sha256']) ==
                (batch['version'], batch['sourceSha256'], batch['installerSha256']),
                'A18 kit release metadata does not match the handoff')
    return expected


def verify_return_metadata(data, kit):
    def result(name):
        return json.loads(data[name])

    device, install, cleanup = (result(name) for name in ('device.json', 'install.json', 'cleanup.json'))
    modules = device['memoryModuleBytes']
    require(isinstance(modules, list) and modules
            and all(type(size) is int and size > 0 for size in modules)
            and sum(modules) == device['installedMemoryBytes'], 'physical memory modules disagree with device.json')
    scratch = PureWindowsPath(data['scratch.path'].decode('utf-8-sig').strip())
    scratch_root = PureWindowsPath(device['scratchRoot'])
    require(scratch_root.is_absolute() and re.fullmatch(r'[A-Z]:', scratch_root.drive, re.I)
            and scratch_root.drive.casefold() == device['scratchDrive'].casefold()
            and '..' not in scratch_root.parts and scratch.parent == scratch_root
            and re.fullmatch(r'ZA18Run-[a-f0-9]{12}', scratch.name)
            and PureWindowsPath(result('candidate-performance.json')['executable'])
            == scratch / 'installed' / 'zhiye-desktop.exe', 'measurement paths disagree with the recorded local SSD')
    require(install['runMode'] in ('reference', 'supplemental') and install['phases'] == dict.fromkeys(PHASES, 0)
            and not install.get('error'), 'install.json has incomplete phases or an invalid run mode')
    require(device['recorded'] == install['started'] and cleanup['removed'] is True
            and datetime.fromisoformat(install['started']) <= datetime.fromisoformat(install['ended'])
            <= datetime.fromisoformat(cleanup['at']), 'cleanup.json does not complete this installation run')
    fixture = json.loads((kit / 'fixture-inputs.json').read_bytes())
    fixture['files'] = {name: sha256(data[name]).hexdigest() for name in ('baseline-core.json', 'pressure-core.json')}
    require(result('fixture-identity.json') == fixture, 'fixture receipt does not match the kit and raw fixtures')


def verify(kit_archive, evidence, handoff_archive=None):
    data = {}
    for name in REPORT_FILES:
        path = evidence / name
        require(path.is_file() and path.resolve() == path, f'missing or redirected evidence file: {name}')
        data[name] = path.read_bytes()
    original = json.loads(data['summary.json'])
    batch = handoff_batch(handoff_archive) if handoff_archive is not None else None
    temporary_parent = Path(gettempdir()).resolve(strict=True)
    with TemporaryDirectory(prefix='zhiye-a18-audit-', dir=temporary_parent) as workspace:
        root = Path(workspace).resolve(strict=True)
        require(root.parent == temporary_parent and root.name.startswith('zhiye-a18-audit-'),
                'unexpected A18 verification scratch location')
        kit, results = root / 'kit', root / 'results'
        kit_sha = load_kit(kit_archive, kit, batch)
        verify_return_metadata(data, kit)
        results.mkdir()
        for name, content in data.items():
            if name != 'summary.json':
                (results / name).write_bytes(content)
        replay = subprocess.run(
            [str(kit / 'tools/node.exe'), str(kit / 'audit-run.mjs'), str(results), str(kit / 'packages.json')],
            cwd=root, capture_output=True, encoding='utf-8', errors='replace', check=False,
        )
        require(replay.returncode == 0, f'raw A18 audit failed\n{replay.stderr.strip()}')
        recovered = json.loads((results / 'summary.json').read_bytes())
        require(recovered == original, 'summary.json differs from the replayed audit')

    mode = recovered['runMode']
    reference = recovered['referenceCriteriaMet']
    require(mode != 'reference' or reference, 'reference run did not pass the hardware and performance gates')
    report = {
        'passed': recovered['passed'], 'classification': mode, 'referenceCriteriaMet': reference,
        'version': recovered['version'], 'kitSha256': kit_sha, 'sourceSha256': recovered['sourceSha256'],
        'installerSha256': recovered['installerSha256'], 'applicationSha256': recovered['applicationSha256'],
        'thresholds': recovered['thresholds'], 'httpRequests': recovered['http']['requests'],
        'pressure': recovered['pressure'], 'evidenceSha256': {name: sha256(raw).hexdigest() for name, raw in data.items()},
    }
    if batch is not None:
        require(all(report[key] == value for key, value in batch.items() if key != 'handoffArchiveSha256'),
                'replayed A18 evidence does not match the handoff')
        report['handoffArchiveSha256'] = batch['handoffArchiveSha256']
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('evidence', type=Path, help='directory returned by Run-A18.ps1')
    parser.add_argument('--kit', type=Path, required=True, help='released A18 ZIP with its .sha256 beside it')
    parser.add_argument('--handoff', type=Path, help='Windows handoff ZIP with its .sha256 beside it')
    parser.add_argument('--allow-supplemental', action='store_true', help='return success for a valid supplemental run')
    args = parser.parse_args()
    try:
        handoff = args.handoff.resolve(strict=True) if args.handoff is not None else None
        result = verify(args.kit.resolve(strict=True), args.evidence.resolve(strict=True), handoff)
    except (OSError, ValueError, TypeError, KeyError, BadZipFile) as error:
        print(f'A18 evidence rejected: {error}', file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2))
    return 0 if result['referenceCriteriaMet'] or args.allow_supplemental else 2


if __name__ == '__main__':
    sys.exit(main())

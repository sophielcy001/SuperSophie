# SPDX-License-Identifier: AGPL-3.0-or-later

"""Exercise the source archive entry point with synthetic project files."""

from contextlib import redirect_stdout
from hashlib import sha256
import importlib.util
from io import StringIO
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
from zipfile import ZipFile


SCRIPT = Path(__file__).resolve().with_name('package-source.py')
SPEC = importlib.util.spec_from_file_location('source_packager', SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
VERSION = '0.50.14'
PREFIX = f'Zhiye-{VERSION}-source/'


class SourceArchiveTests(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory(prefix='zhiye-source-test-')
        self.addCleanup(temporary.cleanup)
        self.scratch = Path(temporary.name).resolve()
        self.project = self.scratch / 'project'
        self.output = self.scratch / 'source.zip'
        self.add('package.json', json.dumps({'version': VERSION}).encode())
        self.add('docs/页面.md', '合成项目资料\n'.encode())
        self.add('docs/ignored.bin', b'not a source file')
        self.add('docs/验收记录/ignored.md', b'local acceptance notes')

    def add(self, relative, data):
        path = self.project / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def pack(self):
        stdout = StringIO()
        with patch.multiple(MODULE, ROOT=self.project, FILES=('package.json',), TREES=('docs',)), \
                patch.object(sys, 'argv', [str(SCRIPT), str(self.output)]), redirect_stdout(stdout):
            MODULE.main()
        return stdout.getvalue().strip()

    def make_junction(self, target):
        neighbor = self.scratch / 'neighbor'
        neighbor.mkdir()
        (neighbor / 'fixture.md').write_bytes(b'synthetic neighboring fixture')
        subprocess.run(['cmd.exe', '/d', '/c', 'mklink', '/j', str(target), str(neighbor)],
                       check=True, capture_output=True)
        self.addCleanup(target.rmdir)
        return neighbor

    def test_valid_package_keeps_manifest_and_checksum_output(self):
        result = self.pack()
        self.assertEqual(result, f'{sha256(self.output.read_bytes()).hexdigest()}  {self.output.name}')
        with ZipFile(self.output) as archive:
            manifest = json.loads(archive.read(PREFIX + 'SOURCE-MANIFEST.json'))
            self.assertEqual(manifest['version'], VERSION)
            self.assertEqual(set(manifest['sha256']), {'package.json', 'docs/页面.md'})
            for name, expected in manifest['sha256'].items():
                self.assertEqual(sha256(archive.read(PREFIX + name)).hexdigest(), expected)

    @unittest.skipUnless(os.name == 'nt', 'NTFS junctions are Windows-specific')
    def test_rejects_a_junction_within_a_source_tree(self):
        neighbor = self.make_junction(self.project / 'docs' / 'junction')
        with self.assertRaisesRegex(ValueError, 'regular project directory'):
            self.pack()
        self.assertFalse(self.output.exists())
        self.assertEqual((neighbor / 'fixture.md').read_bytes(), b'synthetic neighboring fixture')

    @unittest.skipUnless(os.name == 'nt', 'NTFS junctions are Windows-specific')
    def test_rejects_a_junction_as_a_source_tree_root(self):
        (self.project / 'docs').rename(self.project / 'original-docs')
        self.make_junction(self.project / 'docs')
        with self.assertRaisesRegex(ValueError, 'regular project directory'):
            self.pack()
        self.assertFalse(self.output.exists())

    def test_directory_read_failure_stops_packaging(self):
        def inaccessible_tree(*_args, onerror=None, **_kwargs):
            if onerror is not None:
                onerror(PermissionError('synthetic unreadable source directory'))
            return iter(())

        with patch.object(MODULE.os, 'walk', side_effect=inaccessible_tree), \
                self.assertRaisesRegex(PermissionError, 'unreadable source directory'):
            self.pack()
        self.assertFalse(self.output.exists())

    def test_partial_zip_write_is_removed_and_can_be_retried(self):
        real_write = MODULE.write_entry

        def interrupted(archive, name, content):
            if name.endswith('/package.json'):
                raise OSError('synthetic interrupted write')
            real_write(archive, name, content)

        with patch.object(MODULE, 'write_entry', side_effect=interrupted), \
                self.assertRaisesRegex(OSError, 'synthetic interrupted write'):
            self.pack()
        self.assertFalse(self.output.exists())
        self.pack()
        self.assertTrue(self.output.is_file())

    def test_rejected_zip_is_removed_after_readback(self):
        with patch.object(ZipFile, 'testzip', return_value='unreadable'), \
                self.assertRaisesRegex(ValueError, 'Corrupted ZIP member'):
            self.pack()
        self.assertFalse(self.output.exists())

    def test_existing_output_is_preserved(self):
        self.output.write_bytes(b'previous archive')
        with self.assertRaises(FileExistsError):
            self.pack()
        self.assertEqual(self.output.read_bytes(), b'previous archive')


if __name__ == '__main__':
    unittest.main(verbosity=2)

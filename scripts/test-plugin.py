#!/usr/bin/env python3
"""Archive privacy, permissions and version checks; build the plugin first."""
import hashlib
from pathlib import Path
import runpy
import shutil
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BUILD = runpy.run_path(str(ROOT / 'scripts/build-plugin.py'))['build']
BINARY = (Path(sys.argv[1]) if len(sys.argv) > 1 else
          next((ROOT / 'target/plugin-build').glob('*/release/traversiq'))).resolve()


class PluginArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='traversiq package test ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for folder in ['plugin', 'packaging']:
            shutil.copytree(ROOT / folder, self.root / folder)
        for path in ['Cargo.toml', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'PROVENANCE.md']:
            shutil.copyfile(ROOT / path, self.root / path)

    def test_archive_excludes_unlisted_files_and_keeps_executable_permissions(self):
        (self.root / 'plugin/private.env').write_text('PRIVATE_CANARY')
        (self.root / 'docs').mkdir()
        (self.root / 'docs/private.md').write_text('PRIVATE_CANARY')
        archive = BUILD(self.root, BINARY)
        checksum = archive.with_suffix('.zip.sha256').read_text().split()[0]
        self.assertEqual(checksum, hashlib.sha256(archive.read_bytes()).hexdigest())
        with zipfile.ZipFile(archive) as bundle:
            names = bundle.namelist()
            self.assertEqual(len(names), 14)
            self.assertFalse(any('private' in name or '/docs/' in name for name in names))
            binary = next(name for name in names if name.endswith('/bin/traversiq'))
            self.assertEqual(bundle.read(binary), BINARY.read_bytes())
            self.assertEqual((bundle.getinfo(binary).external_attr >> 16) & 0o777, 0o755)
            self.assertTrue(any(name.endswith('/.agents/plugins/marketplace.json') for name in names))
            self.assertTrue(any(name.endswith('/.claude-plugin/marketplace.json') for name in names))
            self.assertTrue(any(name.endswith('/THIRD_PARTY_NOTICES.md') for name in names))

    def test_mismatched_versions_and_private_build_paths_are_rejected(self):
        manifest = self.root / 'plugin/plugin.json'
        manifest.write_text(manifest.read_text().replace('0.1.0', '999.0.0'))
        with self.assertRaisesRegex(ValueError, 'version differs'):
            BUILD(self.root, BINARY)
        shutil.copyfile(ROOT / 'plugin/plugin.json', manifest)
        dirty = self.root / 'dirty-binary'
        dirty.write_bytes(str(Path.home()).encode())
        with self.assertRaisesRegex(ValueError, 'home path'):
            BUILD(self.root, dirty)


if __name__ == '__main__':
    unittest.main(argv=[sys.argv[0]])

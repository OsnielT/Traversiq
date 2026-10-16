#!/usr/bin/env python3
"""Exercise the real shell installer offline; host commands are always mocked."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='traversiq install test ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.env = dict(os.environ, PATH=str(self.bin), XDG_DATA_HOME=str(self.root / 'data'),
                        FIXTURE=str(self.root), PLATFORM='Darwin:arm64')
        for command in ('awk', 'cat', 'chmod', 'mkdir', 'mktemp', 'mv', 'rm', 'rmdir', 'unzip', 'shasum'):
            (self.bin / command).symlink_to(shutil.which(command))
        self.mock('uname', "print(os.environ['PLATFORM'].split(':')[0 if sys.argv[1] == '-s' else 1])")
        self.mock('curl', """
url = sys.argv[sys.argv.index('-o') - 1]
assert url.startswith('https://github.com/OsnielT/Traversiq/releases/download/v' + os.environ['VERSION'] + '/')
source = Path(os.environ['FIXTURE']) / url.rsplit('/', 1)[1]
if not source.exists(): sys.exit(22)
Path(sys.argv[sys.argv.index('-o') + 1]).write_bytes(source.read_bytes())
""")
        for host in ('codex', 'claude'):
            self.mock(host, """
with (Path(os.environ['FIXTURE']) / 'calls').open('a') as log:
    log.write(json.dumps([Path(sys.argv[0]).name, *sys.argv[1:]]) + '\\n')
sys.exit(int(os.environ.get('HOST_FAILURE', '0')))
""")
        self.env['VERSION'] = VERSION

    def mock(self, name, body):
        path = self.bin / name
        path.write_text(f'#!{sys.executable}\nimport json, os, sys\nfrom pathlib import Path\n' + body + '\n')
        path.chmod(0o755)

    def archive(self, target='aarch64-apple-darwin', incomplete=False):
        name = f'traversiq-{VERSION}-{target}'
        archive = self.root / f'{name}.zip'
        with zipfile.ZipFile(archive, 'w') as bundle:
            paths = ['.agents/plugins/marketplace.json', '.claude-plugin/marketplace.json']
            if not incomplete:
                paths.append('plugins/traversiq/bin/traversiq')
            for path in paths:
                bundle.writestr(f'{name}/{path}', 'fixture')
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_suffix('.zip.sha256').write_text(f'{digest}  {archive.name}\n')
        return self.root / 'data/traversiq' / name

    def run_installer(self, host='codex'):
        return subprocess.run(['/bin/sh', str(ROOT / 'scripts/install.sh'), host],
                              cwd=self.root, env=self.env, text=True, capture_output=True)

    def calls(self):
        path = self.root / 'calls'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def test_platforms_and_both_host_commands(self):
        for platform, target, host in (
            ('Darwin:arm64', 'aarch64-apple-darwin', 'codex'),
            ('Darwin:x86_64', 'x86_64-apple-darwin', 'claude'),
            ('Linux:x86_64', 'x86_64-unknown-linux-gnu', 'codex'),
            ('Linux:aarch64', 'aarch64-unknown-linux-gnu', 'claude'),
        ):
            with self.subTest(platform=platform):
                self.env['PLATFORM'] = platform
                installed = self.archive(target)
                result = self.run_installer(host)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.calls()[-2:], [
                    [host, 'plugin', 'marketplace', 'add', str(installed)],
                    [host, 'plugin', 'add' if host == 'codex' else 'install', 'traversiq@traversiq-local'],
                ])
                self.assertTrue(os.access(installed / 'plugins/traversiq/bin/traversiq', os.X_OK))
                self.assertFalse((installed.parent / '.installing').exists())

    def test_retries_preserve_existing_install_and_config(self):
        installed = self.archive()
        self.assertEqual(self.run_installer().returncode, 0)
        private_config = installed.parent / 'user-config.json'
        private_config.write_text('keep me')
        binary = installed / 'plugins/traversiq/bin/traversiq'
        before = binary.stat().st_mtime_ns
        self.assertEqual(self.run_installer('claude').returncode, 0)
        self.assertEqual(binary.stat().st_mtime_ns, before)
        self.assertEqual(private_config.read_text(), 'keep me')
        (installed / '.archive-sha256').write_text('different')
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Existing installation differs', result.stderr)
        self.assertEqual(binary.stat().st_mtime_ns, before)

    def test_bad_downloads_never_register_a_plugin(self):
        for failure in ('checksum', 'missing', 'incomplete', 'malformed'):
            with self.subTest(failure=failure):
                installed = self.archive(incomplete=failure == 'incomplete')
                archive = next(self.root.glob('*.zip'))
                if failure == 'checksum':
                    with archive.open('ab') as stream: stream.write(b'corruption')
                elif failure == 'missing':
                    archive.unlink()
                elif failure == 'malformed':
                    archive.with_suffix('.zip.sha256').write_text('not a checksum')
                result = self.run_installer()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.calls(), [])
                self.assertFalse(installed.exists())
                self.assertFalse((installed.parent / '.installing').exists())
                self.assertEqual(list(installed.parent.glob('.download.*')), [])

    def test_unsupported_platform_and_host_fail_before_download(self):
        self.env['PLATFORM'] = 'Windows:AMD64'
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertNotEqual(self.run_installer('other').returncode, 0)
        self.assertFalse((self.root / 'data').exists())
        self.assertEqual(self.calls(), [])

    def test_host_failure_retains_registered_path_for_retry(self):
        installed = self.archive()
        self.env['HOST_FAILURE'] = '17'
        self.assertEqual(self.run_installer().returncode, 17)
        self.assertTrue(installed.exists())
        self.assertEqual(len(self.calls()), 1)
        self.assertFalse((installed.parent / '.installing').exists())

    def test_sha256sum_is_supported(self):
        self.archive()
        self.mock('sha256sum', "import hashlib; print(hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest() + '  ' + sys.argv[1])")
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_lock_and_missing_host_leave_existing_state_intact(self):
        self.archive()
        lock = self.root / 'data/traversiq/.installing'
        lock.mkdir(parents=True)
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Another install is running', result.stderr)
        self.assertTrue(lock.exists())
        (self.bin / 'codex').unlink()
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Install codex first', result.stderr)
        self.assertEqual(self.calls(), [])

    def test_pinned_version_matches_manifest(self):
        self.assertIn(f'VERSION={VERSION}\n', (ROOT / 'scripts/install.sh').read_text())


if __name__ == '__main__':
    unittest.main()

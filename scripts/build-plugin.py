#!/usr/bin/env python3
"""Package the native release binary and an explicit list of public plugin files.

Requires Python 3.11+ and the Rust toolchain used to build the binary. Produces
an archive for this host only; it neither installs plugins nor downloads tools.
"""
import hashlib
import json
import os
import shlex
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tomllib
import zipfile

PLUGIN_FILES = (
    'plugin.json', '.claude-plugin/plugin.json', 'mcp.json', 'hooks/hooks.json',
    'skills/setup/SKILL.md', 'skills/impact/SKILL.md',
)


def build(root, binary=None):
    version = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
    for manifest in ('plugin/plugin.json', 'plugin/.claude-plugin/plugin.json'):
        if json.loads((root / manifest).read_text())['version'] != version:
            raise ValueError(f'{manifest}: version differs from Cargo.toml')
    cargo = Path(shutil.which('cargo') or Path.home() / '.cargo/bin/cargo')
    rustc = shutil.which('rustc') or str(cargo.with_name('rustc'))
    metadata = subprocess.check_output([rustc, '-vV'], text=True)
    target = next(line.removeprefix('host: ') for line in metadata.splitlines() if line.startswith('host: '))
    if 'windows' in target:
        raise ValueError('This first plugin package supports Unix hosts only.')
    if binary is None:
        env = os.environ.copy()
        flags = (env['CARGO_ENCODED_RUSTFLAGS'].split('\x1f') if env.get('CARGO_ENCODED_RUSTFLAGS')
                 else shlex.split(env.get('RUSTFLAGS', '')))
        flags.extend([f'--remap-path-prefix={Path.home()}=/source/home',
                      f'--remap-path-prefix={root}=/source/traversiq'])
        env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(flags)
        build_dir = root / 'target/plugin-build'
        subprocess.run([str(cargo), 'build', '--locked', '--release', '--target', target,
                        '--target-dir', str(build_dir)], cwd=root, env=env, check=True)
        binary = build_dir / target / 'release/traversiq'
    binary = binary.resolve(strict=True)
    if not binary.is_file():
        raise ValueError('Build the release binary with cargo build --locked --release first.')
    if str(Path.home()).encode() in binary.read_bytes():
        raise ValueError('Binary contains the build-machine home path; use the default remapped packaging build.')
    name = f'traversiq-{version}-{target}'
    output = root / 'target/plugins'
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f'{name}.zip'
    files = {f'plugins/traversiq/{path}': root / 'plugin' / path for path in PLUGIN_FILES}
    files.update({f'plugins/traversiq/{path}': root / path for path in
                  ('LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'PROVENANCE.md')})
    files.update({
        'plugins/traversiq/bin/traversiq': binary,
        '.agents/plugins/marketplace.json': root / 'packaging/codex-marketplace.json',
        '.claude-plugin/marketplace.json': root / 'packaging/claude-marketplace.json',
        'README.md': root / 'packaging/README.md',
    })
    for path in files.values():
        if path.is_symlink() or not path.is_file():
            raise ValueError(f'Expected a regular public file: {path}')
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as bundle:
        for relative, path in sorted(files.items()):
            info = zipfile.ZipInfo.from_file(path, f'{name}/{relative}')
            info.compress_type = zipfile.ZIP_DEFLATED
            mode = 0o755 if path == binary else 0o644
            info.external_attr = (stat.S_IFREG | mode) << 16
            bundle.writestr(info, path.read_bytes())
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix('.zip.sha256').write_text(f'{checksum}  {archive.name}\n')
    return archive


if __name__ == '__main__':
    root = Path(__file__).resolve().parents[1]
    binary = Path(sys.argv[1]) if len(sys.argv) > 1 else None
    print(build(root, binary))

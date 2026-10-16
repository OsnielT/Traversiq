#!/usr/bin/env python3
"""Check Cargo's source-package file list for known private material (not a full secret scan)."""
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess


def problems(path, data, private_ids=()):
    parts = PurePosixPath(path).parts
    issues = []
    if not parts or parts[0] in {'private', 'target', 'trial-results', 'setup-proposal', 'docs', '.codex', '.git'}:
        issues.append('private/generated path')
    if path in {'DESIGN.md', 'licenses/Apache-2.0.txt', '.mcp.json', 'graphify.local.json', 'graphify.workspace.json', 'graphify-explained.md', 'traversiq.local.json', 'traversiq.workspace.json'} or path.endswith(('.docx', '.sqlite3', '.sqlite3-wal', '.sqlite3-shm', '.sqlite3-journal')):
        issues.append('local configuration/research/cache/export')
    text = data.decode('utf-8', errors='replace')
    if re.search(r'/(?:Users|home)/[^/\s"\']+', text):
        issues.append('absolute home-directory reference')
    if re.search(r'-----BEGIN [A-Z ]*PRIVATE KEY-----', text):
        issues.append('private-key marker')
    if any(project.casefold() in text.casefold() for project in private_ids):
        issues.append('local trial-project identifier')
    return issues


def main():
    root = Path(__file__).resolve().parents[1]
    cargo = shutil.which('cargo') or str(Path.home() / '.cargo/bin/cargo')
    listed = subprocess.check_output([cargo, 'package', '--list', '--allow-dirty', '--offline'], cwd=root, text=True)
    private_ids = set()
    for name in ['graphify.workspace.json', 'traversiq.workspace.json']:
        private_config = root / name
        if private_config.exists():
            private_ids.update(json.loads(private_config.read_text()).get('projects', {}))
    files = listed.splitlines()
    failures = []
    for path in files:
        if path == '.cargo_vcs_info.json':
            continue  # Cargo generates commit provenance rather than copying a local file.
        source = root / path
        # Cargo generates this copy of the original manifest in its package.
        if path == 'Cargo.toml.orig' and not source.exists():
            source = root / 'Cargo.toml'
        issues = problems(path, source.read_bytes(), private_ids)
        if issues:
            failures.append(f'{path}: {", ".join(issues)}')
    required = {'LICENSE', 'README.md', 'NOTICE', 'PROVENANCE.md', 'THIRD_PARTY_NOTICES.md', 'licenses/inventory.json',
                'examples/demo/workspace.json', 'examples/demo/local.json', 'src/main.rs'}
    failures.extend('Missing public file: ' + path for path in sorted(required.difference(files)))
    if failures:
        raise SystemExit('\n'.join(failures))
    print(f'Checked {len(files)} Cargo source-package entries; no known private material found by this limited check.')
    print('This limited check is not comprehensive secret/provenance review or name clearance; binary/platform notices require separate review.')


if __name__ == '__main__':
    main()

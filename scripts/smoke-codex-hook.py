#!/usr/bin/env python3
"""Opt-in installed-Codex test using a loopback mock and temporary demo files.

Trust bypass applies only to this invocation's reviewed fixture hook. Normal
workspace-write sandboxing and approval policy remain enabled. No real model
is called. Requires Codex's bundled catalog and command-hook support (0.163.0).
"""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading


def main():
    root = Path(__file__).resolve().parents[1]
    binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / 'target/debug/traversiq'
    if not binary.is_file():
        raise SystemExit('Build Traversiq first with cargo build --locked, or pass the binary path.')
    # Use shipped metadata and a temporary catalog, never the user's model cache.
    bundled = json.loads(subprocess.check_output(['codex', 'debug', 'models', '--bundled'], text=True))
    metadata = {**bundled['models'][0], 'slug': 'traversiq-mock',
                'display_name': 'Local checkpoint fixture',
                'apply_patch_tool_type': 'freeform', 'experimental_supported_tools': []}
    metadata.pop('tool_mode', None)
    with tempfile.TemporaryDirectory(prefix='traversiq-codex-smoke-') as temp:
        demo = Path(temp) / 'demo'
        shutil.copytree(root / 'examples/demo', demo,
                        ignore=shutil.ignore_patterns('*.sqlite3*', '*.traversiq-checkpoint.json*'))
        catalog = Path(temp) / 'models.json'
        catalog.write_text(json.dumps({'models': [metadata]}))
        target = demo / 'ui/src/Button.tsx'
        before = target.read_text()
        after = before + '\n// checkpoint runtime smoke\n'
        patch = ('*** Begin Patch\n*** Update File: ui/src/Button.tsx\n@@\n'
                 + ''.join(' ' + line + '\n' for line in before.splitlines())
                 + '+\n+// checkpoint runtime smoke\n*** End Patch')
        requests, errors = [], []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                try:
                    data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    requests.append(data)
                    n = len(requests)
                    assert n <= 3, 'unexpected extra model request'
                    if n < 3:
                        assert target.read_text() == before, 'edit happened before the briefing'
                        if n == 2:
                            outputs = [x.get('output', '') for x in data.get('input', [])
                                       if x.get('type') == 'custom_tool_call_output']
                            assert any('Traversiq pre-edit briefing' in str(x) for x in outputs), \
                                'denial did not reach the model'
                        item = {'type': 'custom_tool_call', 'id': f'item_{n}',
                                'call_id': f'call_{n}', 'name': 'apply_patch', 'input': patch}
                    else:
                        assert target.read_text() == after, 'identical retry did not edit'
                        item = {'type': 'message', 'id': 'msg_3', 'role': 'assistant',
                                'content': [{'type': 'output_text', 'text': 'Checkpoint smoke complete.'}]}
                    response = {'id': f'resp_{n}', 'object': 'response', 'created_at': 0,
                                'model': data.get('model'), 'status': 'completed', 'output': [item],
                                'usage': {'input_tokens': 1, 'output_tokens': 1, 'total_tokens': 2}}
                    events = [
                        ('response.created', {'response': {**response, 'status': 'in_progress', 'output': []}}),
                        ('response.output_item.added', {'output_index': 0, 'item': item}),
                        ('response.output_item.done', {'output_index': 0, 'item': item}),
                        ('response.completed', {'response': response}),
                    ]
                    body = ''.join('event: ' + name + '\ndata: '
                                   + json.dumps({'type': name, **payload}) + '\n\n'
                                   for name, payload in events).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except Exception as error:
                    errors.append(str(error))
                    self.send_error(400, str(error))

        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        command = ' '.join(shlex.quote(str(p)) for p in
                           [binary, 'pre-edit', demo / 'workspace.json', demo / 'local.json'])
        hook = ('hooks={PreToolUse=[{matcher="^apply_patch$",hooks=[{type="command",command='
                + json.dumps(command) + ',timeout=10}]}]}')
        args = ['codex', '-a', 'never', 'exec', '--ignore-user-config', '--ignore-rules',
                '--ephemeral', '--skip-git-repo-check', '--dangerously-bypass-hook-trust',
                '-s', 'workspace-write', '-C', str(demo), '--json',
                '-c', 'model_provider="traversiq_mock"', '-c', 'model="traversiq-mock"',
                '-c', 'model_catalog_json=' + json.dumps(str(catalog)),
                '-c', f'model_providers.traversiq_mock={{name="Local hook fixture",base_url="http://127.0.0.1:{server.server_port}/v1",wire_api="responses",requires_openai_auth=false}}',
                '-c', 'mcp_servers={}', '-c', 'features.hooks=true',
                '-c', 'features.apps=false', '-c', 'features.plugins=false',
                '-c', 'web_search="disabled"', '-c', hook,
                'Apply the synthetic UI patch; review the briefing and retry the identical proposal.']
        try:
            output = subprocess.run(args, stdin=subprocess.DEVNULL, capture_output=True,
                                    text=True, timeout=45)
            if output.returncode:
                raise RuntimeError(output.stdout[-4000:] + output.stderr[-4000:])
            assert len(requests) == 3 and not errors, errors
            assert target.read_text() == after
            print('PASS: Codex blocked the first patch, delivered the briefing, and applied the identical retry.')
        finally:
            server.shutdown()
            server.server_close()


if __name__ == '__main__':
    main()

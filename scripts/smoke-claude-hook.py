#!/usr/bin/env python3
"""Opt-in Claude Edit/Write check with isolated state and a loopback mock API.

Verified with Claude Code 2.1.287. No permission bypass or real model call;
only temporary demo files are edited. Requires command hooks and prompt_id.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import threading


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', nargs='?', type=Path)
    parser.add_argument('--plugin', type=Path, help='Load an extracted package as a session-local plugin.')
    options = parser.parse_args()
    plugin = options.plugin.resolve() if options.plugin else None
    binary = (plugin / 'bin/traversiq' if plugin else options.binary or root / 'target/debug/traversiq').resolve()
    if not binary.is_file():
        raise SystemExit('Build Traversiq with cargo build --locked, or pass the binary path.')
    with tempfile.TemporaryDirectory(prefix='traversiq-claude-smoke-') as temp:
        demo = (Path(temp) / 'demo').resolve()
        shutil.copytree(root / 'examples/demo', demo,
                        ignore=shutil.ignore_patterns('*.sqlite3*', '*.traversiq-checkpoint.json*'))
        target = demo / 'ui/src/Button.tsx'
        created = demo / 'ui/src/New.ts'
        before = target.read_text()
        after = before.replace('Example', 'Reviewed example')
        edit = {'file_path': str(target), 'old_string': 'Example',
                'new_string': 'Reviewed example', 'replace_all': False}
        write = {'file_path': str(created), 'content': 'export const value = 1;\n'}
        if plugin:
            subprocess.run([str(binary), 'plugin', 'configure', 'workspace.json', 'local.json'],
                           cwd=demo, check=True, stdout=subprocess.DEVNULL)
        calls = [('Read', {'file_path': str(target)}), ('Edit', edit), ('Edit', edit),
                 ('Write', write), ('Write', write)]
        requests, errors = [], []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                try:
                    data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    if self.path.startswith('/v1/messages/count_tokens'):
                        body = json.dumps({'input_tokens': 10}).encode()
                        content_type = 'application/json'
                    else:
                        assert self.path.startswith('/v1/messages'), self.path
                        requests.append(data)
                        n = len(requests)
                        step = n - bool(plugin)
                        assert n <= 6 + bool(plugin), 'too many model requests'
                        assert target.read_text() == (before if step <= 3 else after), \
                            'Edit executed before its briefing or its retry failed'
                        if step <= 5:
                            assert not created.exists(), 'Write executed before its briefing'
                        else:
                            assert created.read_text() == write['content'], 'Write retry failed'
                        if step in [3, 5]:
                            results = [c for m in data['messages'] for c in m.get('content', [])
                                       if isinstance(c, dict) and c.get('type') == 'tool_result']
                            assert 'Traversiq pre-edit briefing' in json.dumps(results[-1]), \
                                'briefing did not reach the model'
                        if plugin and step == 1:
                            results = [c for m in data['messages'] for c in m.get('content', [])
                                       if isinstance(c, dict) and c.get('type') == 'tool_result']
                            assert all(project in json.dumps(results[-1]) for project in ['admin', 'api', 'ui', 'web']), 'packaged MCP workspace call failed'
                        if step <= 5:
                            if step == 0:
                                names = [t['name'] for t in data['tools'] if t.get('name', '').endswith('__traversiq_workspace')]
                                assert len(names) == 1, 'packaged MCP server was not discovered'
                                name, tool_input = names[0], {}
                            else:
                                name, tool_input = calls[step - 1]
                            block = {'type': 'tool_use', 'id': f'toolu_fixture_{n}',
                                     'name': name, 'input': {}}
                            delta = {'type': 'input_json_delta', 'partial_json': json.dumps(tool_input)}
                            stop = 'tool_use'
                        else:
                            block = {'type': 'text', 'text': ''}
                            delta = {'type': 'text_delta', 'text': 'Checkpoint smoke complete.'}
                            stop = 'end_turn'
                        message = {'id': f'msg_{n}', 'type': 'message', 'role': 'assistant',
                                   'model': data['model'], 'content': [], 'stop_reason': None,
                                   'stop_sequence': None, 'usage': {'input_tokens': 1, 'output_tokens': 0}}
                        events = [
                            ('message_start', {'message': message}),
                            ('content_block_start', {'index': 0, 'content_block': block}),
                            ('content_block_delta', {'index': 0, 'delta': delta}),
                            ('content_block_stop', {'index': 0}),
                            ('message_delta', {'delta': {'stop_reason': stop, 'stop_sequence': None},
                                               'usage': {'output_tokens': 1}}),
                            ('message_stop', {}),
                        ]
                        body = ''.join('event: ' + name + '\ndata: '
                                       + json.dumps({'type': name, **payload}) + '\n\n'
                                       for name, payload in events).encode()
                        content_type = 'text/event-stream'
                    self.send_response(200)
                    self.send_header('Content-Type', content_type)
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
        settings = Path(temp) / 'settings.json'
        settings.write_text(json.dumps({'hooks': {'PreToolUse': [
            {'matcher': '^(Edit|Write)$',
             'hooks': [{'type': 'command', 'command': command, 'timeout': 10}]}]}}))
        if plugin:
            settings.write_text('{}')
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(('ANTHROPIC_', 'CLAUDE_', 'CLAUDECODE', 'AWS_', 'GOOGLE_', 'OTEL_'))
               and k.upper() not in {'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY'}}
        env.update({'ANTHROPIC_BASE_URL': f'http://127.0.0.1:{server.server_port}',
                    'ANTHROPIC_API_KEY': 'traversiq-local-fixture',
                    'CLAUDE_CONFIG_DIR': str(Path(temp) / 'claude-state'),
                    'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1',
                    'NO_PROXY': '127.0.0.1,localhost'})
        args = ['claude', '-p', '--restricted', '--setting-sources', '', '--settings', str(settings),
                '--model', 'claude-sonnet-4-5', '--strict-mcp-config',
                '--mcp-config', '{"mcpServers":{}}', '--tools', 'Read,Edit,Write',
                '--permission-mode', 'acceptEdits', '--no-session-persistence',
                '--disable-slash-commands', '--output-format', 'stream-json', '--verbose',
                '--include-hook-events', '--max-turns', '10',
                '--system-prompt', 'Follow the user request using only the allowed file tools.',
                'Edit the demo Button label after reading it, review Traversiq evidence and retry '
                'unchanged edits; also create New.ts.']
        if plugin:
            args.remove('--strict-mcp-config')
            args.extend(['--plugin-dir', str(plugin), '--allowedTools',
                         'mcp__plugin_traversiq_traversiq__traversiq_workspace'])
        try:
            output = subprocess.run(args, cwd=demo, env=env, stdin=subprocess.DEVNULL,
                                    capture_output=True, text=True, timeout=45)
            if output.returncode:
                raise RuntimeError(output.stdout[-4000:] + output.stderr[-4000:])
            assert len(requests) == 6 + bool(plugin) and not errors, errors
            print('PASS: Claude' + (' loaded the packaged MCP server and' if plugin else '')
                  + ' blocked Edit and Write, delivered both briefings, and applied identical retries.')
        finally:
            server.shutdown()
            server.server_close()


if __name__ == '__main__':
    main()

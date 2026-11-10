import argparse
import base64
import gzip
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import threading
import urllib.request

root = Path(__file__).resolve().parents[1]
backend = root / 'crates/backends/claude'
output = root / 'output/claude-upstream-parity'
output.mkdir(parents=True, exist_ok=True)
manifest_path = backend / 'prompts/upstream.json'
manifest = json.loads(manifest_path.read_text())
parser = argparse.ArgumentParser()
parser.add_argument('--capture', action='store_true')
parser.add_argument('--write', action='store_true')
parser.add_argument('--offline', action='store_true')
parser.add_argument('--claude', default=shutil.which('claude'))
args = parser.parse_args()
if args.capture and args.offline:
    raise ValueError('--capture requires official package verification; omit --offline')
if args.write and not args.capture:
    raise ValueError('--write requires a fresh --capture')


def sha256(data):
    return hashlib.sha256(data).hexdigest()


if not args.offline:
    archive_path = output / 'native-package.tgz'
    if not archive_path.exists():
        archive_path.write_bytes(urllib.request.urlopen(manifest['platform_tarball']).read())
    archive = archive_path.read_bytes()
    algorithm, expected = manifest['platform_integrity'].split('-', 1)
    actual = base64.b64encode(hashlib.new(algorithm, archive).digest()).decode()
    if actual != expected:
        raise ValueError('Official platform package integrity differs')
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as package:
        executable = package.extractfile('package/claude').read()
    if sha256(executable) != manifest['executable_sha256']:
        raise ValueError('Official executable digest differs')
    source = executable.decode('utf-8', 'replace')

    def literal(start, values):
        at = source.index(start)
        text = source[at + 1:source.index(start[0], at + 1)].replace('\\u2014', '\u2014')
        for value in values:
            head, _, tail = text.partition('${')
            text = head + value + tail.partition('}')[2]
        return text

    shape = literal('"an <analysis> block followed by a <summary> block"', [])
    constraints = literal('"Note any security-relevant instructions or constraints the user stated', [])
    voices = literal("' Only messages that actually came from the user (user-role turns)", [])
    analysis = literal('`Before providing your final summary, wrap your analysis', [constraints])
    scratchpad = literal('`<analysis>\n[Your thought process', [])
    compact = (literal('`CRITICAL: Respond with TEXT ONLY.', [shape])
               + literal('`Your task is to create a detailed summary of the conversation so far,',
                         ['', '', analysis, voices, scratchpad])
               + literal('`\n\nREMINDER: Do NOT call any tools.', [])
               + literal('`${e}. Tool calls will be rejected and you will fail the task.', [shape]))
    if args.write:
        (backend / 'prompts/compact.md').write_text(compact)
    if (backend / 'prompts/compact.md').read_text() != compact:
        raise ValueError('Upstream compaction prompt differs')
    if args.capture:
        installed = Path(args.claude).resolve()
        if sha256(installed.read_bytes()) != manifest['executable_sha256']:
            raise ValueError('Installed Claude differs from the pinned official executable')


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *values):
        pass

    def do_POST(self):
        data = self.rfile.read(int(self.headers.get('content-length', 0)))
        if self.headers.get('content-encoding') == 'gzip':
            data = gzip.decompress(data)
        body = json.loads(data)
        self.server.records.append({'path': self.path, 'body': body})
        content_type = 'application/json'
        if '/messages' in self.path and 'count_tokens' not in self.path:
            message = {'id': 'msg_capture', 'type': 'message', 'role': 'assistant',
                       'model': body['model'], 'content': [], 'stop_reason': None,
                       'stop_sequence': None, 'usage': {'input_tokens': 1, 'output_tokens': 1}}
            if body.get('stream'):
                events = [('message_start', {'type': 'message_start', 'message': message}),
                          ('content_block_start', {'type': 'content_block_start', 'index': 0,
                                                   'content_block': {'type': 'text', 'text': ''}}),
                          ('content_block_delta', {'type': 'content_block_delta', 'index': 0,
                                                   'delta': {'type': 'text_delta', 'text': 'captured'}}),
                          ('content_block_stop', {'type': 'content_block_stop', 'index': 0}),
                          ('message_delta', {'type': 'message_delta',
                                             'delta': {'stop_reason': 'end_turn', 'stop_sequence': None},
                                             'usage': {'output_tokens': 1}}),
                          ('message_stop', {'type': 'message_stop'})]
                data = ''.join(f'event: {name}\ndata: {json.dumps(value)}\n\n'
                               for name, value in events).encode()
                content_type = 'text/event-stream'
            else:
                message.update(content=[{'type': 'text', 'text': 'captured'}], stop_reason='end_turn')
                data = json.dumps(message).encode()
        else:
            data = json.dumps({'input_tokens': 1}).encode()
        self.send_response(200)
        self.send_header('content-type', content_type)
        self.send_header('content-length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if args.capture:
    for profile, options in manifest['profiles'].items():
        folder = output / profile
        folder.mkdir(exist_ok=True)
        workspace = folder / 'workspace'
        workspace.mkdir(exist_ok=True)
        config = folder / 'config'
        config.mkdir(exist_ok=True)
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        server.records = []
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        env = {k: v for k, v in os.environ.items() if k in ['PATH', 'TMPDIR', 'LANG', 'SHELL', 'TERM']}
        env.update(ANTHROPIC_API_KEY='local-capture-fixture',
                   ANTHROPIC_BASE_URL=f'http://127.0.0.1:{server.server_port}',
                   CLAUDE_CONFIG_DIR=str(config), CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1',
                   DISABLE_AUTOUPDATER='1', ENABLE_TOOL_SEARCH='false')
        command = [str(installed), '-p', 'Return captured.', '--model', manifest['model'],
                   '--no-session-persistence', '--output-format', 'json', '--setting-sources', '',
                   '--settings', '{}', '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}',
                   '--no-chrome', '--disable-slash-commands', '--exclude-dynamic-system-prompt-sections']
        try:
            with (folder / 'stdout.log').open('w') as stdout, (folder / 'stderr.log').open('w') as stderr:
                subprocess.run(command + options, cwd=workspace, env=env, stdout=stdout,
                               stderr=stderr, check=True, timeout=45)
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
        (folder / 'requests.json').write_text(json.dumps(server.records, indent=2) + '\n')

profiles = {}
for profile in manifest['profiles']:
    records = json.loads((output / profile / 'requests.json').read_text())
    profiles[profile] = next(record['body'] for record in records
                             if '/messages' in record['path'] and 'count_tokens' not in record['path'])
blocks = profiles['full']['system'][1:]
if any(request['system'][1:] != blocks for request in profiles.values()):
    raise ValueError('Core system blocks differ across upstream profiles')
catalogs = {name: {tool['name']: tool for tool in request['tools']}
            for name, request in profiles.items()}
tools = [catalogs[manifest['tool_profiles'][name]][name]
         for name in sorted(manifest['tool_profiles'])]
artifacts = {'prompts/claude-code-system.json': blocks, 'tools/src/tools/upstream.json': tools}
for entry in manifest['artifacts']:
    path = backend / entry['path']
    expected = artifacts[entry['path']]
    if args.write:
        data = (json.dumps(expected, ensure_ascii=False, indent=2) + '\n').encode()
        path.write_bytes(data)
        entry['sha256'] = sha256(data)
    data = path.read_bytes()
    if sha256(data) != entry['sha256'] or json.loads(data) != expected:
        raise ValueError(f"Upstream artifact differs: {entry['path']}")
if args.write:
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
report = {'version': manifest['version'], 'system_text_blocks_verified': len(blocks),
          'tool_definitions_verified': len(tools), 'official_package_verified': not args.offline,
          'fresh_capture': args.capture, 'tool_profiles': manifest['tool_profiles']}
(output / 'verification.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))

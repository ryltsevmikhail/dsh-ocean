"""Headless tests for the strictly isolated native GUI child fixture."""
import http.client
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / 'scripts' / 'gui-child-fixture.py'
ARGS = ['--profile', 'web', '--host', '127.0.0.1', '--port', '0', '--no-open']


class GuiChildFixtureTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='dsh-ocean-native-test.', dir='/tmp')
        self.runtime = Path(self.temporary.name) / 'runtime'
        self.root = self.runtime / 'gui-child'
        self.root.mkdir(parents=True, mode=0o700)
        self.env = {'PATH': '/usr/bin:/bin', 'PYTHONDONTWRITEBYTECODE': '1',
                    'DSH_OCEAN_TEST_GUI_CHILD': '1',
                    'DSH_OCEAN_TEST_GUI_CHILD_ROOT': str(self.root),
                    'XDG_RUNTIME_DIR': str(self.runtime)}
        for key, name in [('HOME', 'home'), ('DSH_HOME', 'dsh'),
                          ('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                          ('XDG_STATE_HOME', 'state'), ('XDG_CACHE_HOME', 'cache'),
                          ('DSH_OCEAN_DATA_DIR', 'ocean-data'), ('TMPDIR', 'tmp')]:
            directory = self.root / name
            directory.mkdir(mode=0o700)
            self.env[key] = str(directory)
        self.children = []

    def tearDown(self):
        try:
            for process, sentinel in self.children:
                self.stop(process, sentinel)
        finally:
            self.temporary.cleanup()

    def probe(self, args, env=None):
        return subprocess.run(['/usr/bin/python3', '-B', str(SCRIPT), *args],
                              env=self.env if env is None else env,
                              capture_output=True, timeout=5, start_new_session=True)

    def start(self):
        process = subprocess.Popen(['/usr/bin/python3', '-B', str(SCRIPT), *ARGS],
                                   env=self.env, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, start_new_session=True)
        self.children.append((process, None))
        readable, _, _ = select.select([process.stdout], [], [], 5)
        self.assertTrue(bool(readable), 'fixture startup timed out')
        line = process.stdout.readline().decode()
        match = re.fullmatch(r'dsh web: http://127\.0\.0\.1:(\d+)/\?token=([A-Za-z0-9_-]+)\n', line)
        # Deliberately never include the startup line or token in assertion output.
        self.assertTrue(match is not None, 'fixture did not emit expected startup format')
        port, token = int(match[1]), match[2]
        records = list((self.root / 'servers').glob('*.json'))
        generation = max(int(path.stem) for path in records)
        record_path = self.root / 'servers' / f'{generation}.json'
        metadata = json.loads(record_path.read_text())
        self.children[-1] = (process, metadata['sentinel_pid'])
        self.assertEqual(set(metadata), {'generation', 'pid', 'ppid', 'pgid', 'port',
                                        'sentinel_pid', 'marker_preserved'})
        self.assertEqual(metadata['pid'], process.pid)
        self.assertEqual(metadata['ppid'], os.getpid())
        self.assertEqual(metadata['pgid'], process.pid)
        self.assertEqual(os.getpgid(metadata['sentinel_pid']), process.pid)
        self.assertEqual(metadata['port'], port)
        self.assertNotEqual(port, 3080)
        self.assertEqual(record_path.stat().st_mode & 0o777, 0o600)
        return process, metadata, token

    def stop(self, process, sentinel):
        if process.poll() is None:
            # Only target a session/group created and owned by this test.
            self.assertEqual(os.getpgid(process.pid), process.pid)
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
                self.fail('normal fixture shutdown timed out')
        if sentinel is not None:
            with self.assertRaises(ProcessLookupError):
                os.kill(sentinel, 0)
        process.stdout.close()
        process.stderr.close()

    def request(self, port, method, target='/', headers=None):
        connection = http.client.HTTPConnection('127.0.0.1', port, timeout=5)
        try:
            connection.request(method, target, headers=headers or {})
            response = connection.getresponse()
            return response.status, dict(response.getheaders()), response.read()
        finally:
            connection.close()

    def test_version_is_guarded_and_has_no_side_effects(self):
        before = sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*'))
        result = self.probe(['--version'])
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b'0.2.0\n')
        self.assertEqual(result.stderr, b'')
        self.assertEqual(before, sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*')))
        for key in self.env:
            if key in ('PATH', 'PYTHONDONTWRITEBYTECODE'):
                continue
            with self.subTest(variable=key):
                env = dict(self.env)
                env.pop(key)
                self.assertNotEqual(self.probe(['--version'], env).returncode, 0)
        self.assertFalse((self.root / 'counter').exists())

    def test_guards_and_strict_arguments(self):
        for key in ('HOME', 'DSH_HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME',
                    'XDG_STATE_HOME', 'XDG_CACHE_HOME', 'DSH_OCEAN_DATA_DIR', 'TMPDIR',
                    'DSH_OCEAN_TEST_GUI_CHILD_ROOT', 'XDG_RUNTIME_DIR'):
            with self.subTest(variable=key):
                env = dict(self.env, **{key: '/tmp'})
                self.assertNotEqual(self.probe(ARGS, env).returncode, 0)
        alias = self.root / 'alias'
        alias.symlink_to(self.root / 'home', target_is_directory=True)
        env = dict(self.env, HOME=str(alias))
        self.assertNotEqual(self.probe(ARGS, env).returncode, 0)
        for args in ([], ARGS + ['extra'], ARGS[:-1], ['--version', 'extra'],
                     ['--profile', 'web', '--host', '0.0.0.0', '--port', '0', '--no-open'],
                     ['--profile', 'web', '--host', '127.0.0.1', '--port', '3080', '--no-open']):
            self.assertNotEqual(self.probe(args).returncode, 0)
        self.assertFalse((self.root / 'counter').exists())
        self.assertFalse((self.root / 'servers').exists())

    def test_http_rendering_and_restart_marker(self):
        process, metadata, token = self.start()
        port = metadata['port']
        self.assertEqual(self.request(port, 'GET')[0], 401)
        self.assertEqual(self.request(port, 'GET', '/?token=invalid')[0], 401)
        status, headers, _ = self.request(port, 'GET', '/?token=' + token)
        self.assertEqual(status, 303)
        self.assertEqual(headers.get('Location'), '/')
        self.assertEqual(headers.get('Cache-Control'), 'no-store')
        cookie_header = headers.get('Set-Cookie', '')
        self.assertTrue('HttpOnly' in cookie_header and 'SameSite=Strict' in cookie_header)
        cookie = cookie_header.split(';')[0]
        status, _, body = self.request(port, 'GET', headers={'Cookie': cookie})
        self.assertEqual(status, 200)
        self.assertTrue(f'Generation {metadata["generation"]}'.encode() in body)
        self.assertTrue(b"fetch('/rendered?generation=1', {method:'POST',credentials:'same-origin'})" in body)
        self.assertTrue(b"document.visibilityState!=='visible'" in body)
        self.assertTrue(b"requestAnimationFrame" in body)
        self.assertFalse((self.root / 'rendered').exists())
        origin = f'http://127.0.0.1:{port}'
        for request_headers, expected in [({}, 401), ({'Origin': origin}, 401),
                ({'Cookie': cookie}, 403),
                ({'Cookie': cookie, 'Origin': 'http://example.invalid'}, 403)]:
            self.assertEqual(self.request(port, 'POST', '/rendered?generation=1', request_headers)[0], expected)
        self.assertFalse((self.root / 'rendered').exists())
        self.assertEqual(self.request(port, 'GET', '/rendered?generation=1', {'Cookie': cookie})[0], 404)
        self.assertEqual(self.request(port, 'POST', '/unknown', {'Cookie': cookie})[0], 404)
        self.assertEqual(self.request(port, 'PUT')[0], 501)
        self.assertEqual(self.request(port, 'POST', '/rendered?generation=1',
            {'Cookie': cookie, 'Origin': origin})[0], 204)
        rendered = self.root / 'rendered' / '1.json'
        self.assertEqual(json.loads(rendered.read_text()), {'generation': 1, 'rendered': True})
        self.assertEqual(rendered.stat().st_mode & 0o777, 0o600)
        marker = Path(self.env['DSH_HOME']) / 'gui-marker'
        marker_before = marker.read_bytes()
        self.stop(process, metadata['sentinel_pid'])
        self.children.remove((process, metadata['sentinel_pid']))
        _, second, second_token = self.start()
        self.assertEqual(second['generation'], 2)
        self.assertTrue(second['marker_preserved'])
        self.assertEqual(marker.read_bytes(), marker_before)
        status, second_headers, _ = self.request(second['port'], 'GET', '/?token=' + second_token)
        self.assertEqual(status, 303)
        second_auth = {'Cookie': second_headers['Set-Cookie'].split(';')[0],
                       'Origin': f'http://127.0.0.1:{second["port"]}'}
        # A cached/stale first-generation page must not satisfy the new render.
        self.assertEqual(self.request(second['port'], 'POST', '/rendered?generation=1', second_auth)[0], 404)
        self.assertFalse((self.root / 'rendered' / '2.json').exists())
        self.assertEqual(self.request(second['port'], 'POST', '/rendered?generation=2', second_auth)[0], 204)
        self.assertEqual(json.loads((self.root / 'rendered' / '2.json').read_text()),
                         {'generation': 2, 'rendered': True})

    def test_idle_browser_connection_does_not_block_health_probe(self):
        _, metadata, _ = self.start()
        with socket.create_connection(('127.0.0.1', metadata['port']), timeout=1):
            connection = http.client.HTTPConnection('127.0.0.1', metadata['port'], timeout=1)
            try:
                connection.request('GET', '/')
                response = connection.getresponse()
                self.assertEqual(response.status, 401)
                response.read()
            finally:
                connection.close()

    def test_fifo_marker_rejected_before_binding(self):
        (self.root / 'counter').write_bytes(b'1\n')
        os.mkfifo(Path(self.env['DSH_HOME']) / 'gui-marker', 0o600)
        self.assertNotEqual(self.probe(ARGS).returncode, 0)
        self.assertFalse((self.root / 'servers').exists())

    def test_malformed_counter_and_symlink_refused(self):
        counter = self.root / 'counter'
        for value in (b'broken', b'0\n', b'9' * 100, b'1\nextra'):
            counter.write_bytes(value)
            self.assertNotEqual(self.probe(ARGS).returncode, 0)
            self.assertFalse((self.root / 'servers').exists())
        counter.unlink()
        counter.symlink_to(self.root / 'missing')
        self.assertNotEqual(self.probe(ARGS).returncode, 0)
        self.assertFalse((self.root / 'missing').exists())


if __name__ == '__main__':
    unittest.main()

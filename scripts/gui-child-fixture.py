#!/usr/bin/python3
"""Guarded, local-only GUI child used by native integration tests."""
import fcntl
import hmac
import json
import os
from pathlib import Path
import re
import secrets
import signal
import stat
import subprocess
import sys
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from http.cookies import SimpleCookie
from urllib.parse import parse_qs, urlsplit


class Refusal(Exception):
    pass


def canonical(value):
    if not value:
        raise Refusal()
    path = Path(value)
    if not path.is_absolute() or path.resolve() != path:
        raise Refusal()
    # resolve() also detects symlinks in every ancestor, including dangling ones.
    return path


def guard():
    if os.environ.get('DSH_OCEAN_TEST_GUI_CHILD') != '1':
        raise Refusal()
    runtime = canonical(os.environ.get('XDG_RUNTIME_DIR'))
    if not re.fullmatch(r'/tmp/dsh-ocean-native-test\.[^/]+/runtime', str(runtime)):
        raise Refusal()
    root = canonical(os.environ.get('DSH_OCEAN_TEST_GUI_CHILD_ROOT'))
    if root != runtime / 'gui-child' or not root.is_dir():
        raise Refusal()
    for key in ('HOME', 'DSH_HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME',
                'XDG_STATE_HOME', 'XDG_CACHE_HOME', 'DSH_OCEAN_DATA_DIR', 'TMPDIR'):
        path = canonical(os.environ.get(key))
        if root not in path.parents or not path.is_dir():
            raise Refusal()
    return root


def private_open(path, flags):
    fd = os.open(path, flags | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, 0o600)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        os.close(fd)
        raise Refusal()
    return fd


def atomic_json(directory, name, value):
    directory.mkdir(mode=0o700, exist_ok=True)
    if directory.is_symlink() or not directory.is_dir():
        raise Refusal()
    fd, temporary = tempfile.mkstemp(prefix='.record-', dir=directory)
    try:
        with os.fdopen(fd, 'w') as output:
            json.dump(value, output)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, directory / name)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def allocate(root):
    fd = private_open(root / 'counter', os.O_RDWR | os.O_CREAT)
    with os.fdopen(fd, 'r+b') as counter:
        fcntl.flock(counter, fcntl.LOCK_EX)
        raw = counter.read(33)
        if raw and not re.fullmatch(rb'[1-9][0-9]{0,8}\n', raw):
            raise Refusal()
        generation = int(raw or b'0') + 1
        if generation > 999999999:
            raise Refusal()
        marker = Path(os.environ['DSH_HOME']) / 'gui-marker'
        if generation == 1:
            marker_fd = private_open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL)
            with os.fdopen(marker_fd, 'wb') as output:
                output.write(b'dsh-ocean-gui-fixture-marker-v1\n')
        else:
            marker_fd = private_open(marker, os.O_RDONLY)
            with os.fdopen(marker_fd, 'rb') as source:
                if source.read(128) != b'dsh-ocean-gui-fixture-marker-v1\n':
                    raise Refusal()
        counter.seek(0)
        counter.write(f'{generation}\n'.encode())
        counter.truncate()
        counter.flush()
        os.fsync(counter.fileno())
        return generation


def serve(root):
    generation = allocate(root)
    token = secrets.token_urlsafe(32)
    cookie = secrets.token_urlsafe(32)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def reply(self, status, body=b'', headers=()):
            self.send_response(status)
            for key, value in headers:
                self.send_header(key, value)
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Cache-Control', 'no-store')
            self.end_headers()
            self.wfile.write(body)

        def authenticated(self):
            try:
                values = SimpleCookie(self.headers.get('Cookie', ''))
                return 'gui_session' in values and hmac.compare_digest(
                    values['gui_session'].value, cookie)
            except Exception:
                return False

        def do_GET(self):
            parsed = urlsplit(self.path)
            if parsed.path != '/' or parsed.fragment:
                return self.reply(404)
            if parsed.query:
                values = parse_qs(parsed.query, keep_blank_values=True)
                if set(values) != {'token'} or len(values['token']) != 1 or not hmac.compare_digest(values['token'][0].encode('utf-8'), token.encode('ascii')):
                    return self.reply(401)
                return self.reply(303, headers=(('Location', '/'), ('Set-Cookie',
                    f'gui_session={cookie}; Path=/; HttpOnly; SameSite=Strict')))
            if not self.authenticated():
                return self.reply(401)
            body = (f'<!doctype html><html><body>Generation {generation}'
                    "<script>let sent=false;function report(){"
                    "if(sent||document.visibilityState!=='visible')return;"
                    "requestAnimationFrame(()=>{if(sent||document.visibilityState!=='visible')return;"
                    "sent=true;fetch('/rendered?generation="
                    f"{generation}', "
                    "{method:'POST',credentials:'same-origin'});});}"
                    "document.addEventListener('visibilitychange',report);"
                    "addEventListener('pageshow',report);report();</script>"
                    '</body></html>').encode()
            self.reply(200, body, (('Content-Type', 'text/html; charset=utf-8'),))

        def do_POST(self):
            if self.path != f'/rendered?generation={generation}':
                return self.reply(404)
            if not self.authenticated():
                return self.reply(401)
            if self.headers.get_all('Origin') != [f'http://127.0.0.1:{self.server.server_port}']:
                return self.reply(403)
            atomic_json(root / 'rendered', f'{generation}.json',
                        {'generation': generation, 'rendered': True})
            self.reply(204)

    class Server(ThreadingHTTPServer):
        daemon_threads = True

        def get_request(self):
            connection, address = super().get_request()
            connection.settimeout(2)
            return connection, address

    # WebKit may open speculative idle connections. They must not monopolize
    # root/token exchanges, render beacons or health probes on other sockets.
    server = Server(('127.0.0.1', 0), Handler)
    sentinel = None
    try:
        if server.server_port == 3080:
            raise Refusal()
        sentinel = subprocess.Popen(['/bin/sleep', '120'], close_fds=True)
        atomic_json(root / 'servers', f'{generation}.json', {
            'generation': generation, 'pid': os.getpid(), 'ppid': os.getppid(),
            'pgid': os.getpgrp(), 'port': server.server_port,
            'sentinel_pid': sentinel.pid, 'marker_preserved': True})
        print(f'dsh web: http://127.0.0.1:{server.server_port}/?token={token}', flush=True)
        server.serve_forever()
    finally:
        server.server_close()
        if sentinel is not None:
            sentinel.terminate()
            try:
                sentinel.wait(timeout=2)
            except subprocess.TimeoutExpired:
                sentinel.kill()
                sentinel.wait(timeout=2)


def main():
    try:
        root = guard()
        if sys.argv[1:] == ['--version']:
            print('0.2.0')
            return 0
        if sys.argv[1:] != ['--profile', 'web', '--host', '127.0.0.1', '--port', '0', '--no-open']:
            raise Refusal()
        def terminate(_signum, _frame):
            raise KeyboardInterrupt()
        signal.signal(signal.SIGTERM, terminate)
        serve(root)
        return 0
    except KeyboardInterrupt:
        return 0
    except Exception:
        # Never echo input, environment, request data, or secrets.
        print('gui child fixture refused', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())

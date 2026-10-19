"""Black-box protocol, HTTP, integrity and cancellation tests. Python stdlib only."""
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BINARY = str(Path(sys.argv.pop(1)).resolve())
PAYLOAD = bytes(range(256)) * 16384 + b'pack-end'
DIGEST = hashlib.sha256(PAYLOAD).hexdigest()


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    slow = True

    def handle(self):
        try:
            super().handle()
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass  # A cancelled downloader deliberately closes keep-alive sockets.

    def log_message(self, *_):
        pass

    def do_HEAD(self):
        self.reply(head=True)

    def do_GET(self):
        self.reply()

    def reply(self, head=False):
        try:
            if self.path.startswith('/auth') and self.headers.get('Authorization') != 'Bearer test-secret':
                self.send_response(403)
                self.send_header('Content-Length', '0')
                self.end_headers()
                return
            if self.path.startswith('/redirect'):
                self.send_response(302)
                self.send_header('Location', 'http://localhost:%d/auth' % self.server.server_port)
                self.send_header('Content-Length', '0')
                self.end_headers()
                return
            start, end = 0, len(PAYLOAD) - 1
            partial = 'Range' in self.headers and not self.path.startswith(('/no-range', '/unknown-size'))
            if partial:
                value = self.headers['Range'].removeprefix('bytes=')
                first, last = value.split('-')
                start = int(first)
                end = min(int(last) if last else end, end)
            self.send_response(206 if partial else 200)
            if not self.path.startswith(('/no-range', '/unknown-size')):
                self.send_header('Accept-Ranges', 'bytes')
            self.send_header('ETag', '"fixture-v1"')
            if partial:
                wrong = 1 if self.path.startswith('/bad-range') and end > start else 0
                self.send_header('Content-Range', f'bytes {start + wrong}-{end}/{len(PAYLOAD)}')
            body = PAYLOAD[start:end + 1]
            if not self.path.startswith('/unknown-size'):
                self.send_header('Content-Length', str(len(body)))
            else:
                self.send_header('Connection', 'close')
                self.close_connection = True
            self.end_headers()
            if head:
                return
            for offset in range(0, len(body), 4096):
                self.wfile.write(body[offset:offset + 4096])
                self.wfile.flush()
                if self.path.startswith('/slow') and Handler.slow and len(body) > 1:
                    time.sleep(0.008)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass


class Integration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.base = f'http://127.0.0.1:{cls.server.server_port}'

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.output = Path(self.tmp.name) / '包 with spaces.muapack'

    def request(self, path='/data', **kw):
        return dict(url=self.base + path, output=str(self.output), sha256=DIGEST,
                    expected_size=len(PAYLOAD), **kw)

    def invoke(self, request, command=None):
        data = json.dumps(request) + '\n'
        if command is not None:
            data += json.dumps(command) + '\n'
        result = subprocess.run([BINARY, '--request-stdin'], input=data, text=True,
                                encoding='utf-8', capture_output=True, timeout=25)
        events = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertTrue(events, result.stderr)
        self.assertTrue(all(event['protocol'] == 1 for event in events))
        return result, events

    def assert_success(self, result, events):
        self.assertEqual(result.returncode, 0, (events, result.stderr))
        self.assertEqual(events[-1]['event'], 'complete')
        self.assertEqual(sum(e['event'] == 'complete' for e in events), 1)
        self.assertEqual(events[-1]['sha256'], DIGEST)
        self.assertEqual(self.output.read_bytes(), PAYLOAD)

    def test_parallel_auth_unicode_path_and_json_only(self):
        result, events = self.invoke(self.request('/auth', headers={'Authorization': 'Bearer test-secret'}, connections=4))
        self.assert_success(result, events)
        self.assertNotIn('test-secret', result.stdout + result.stderr)
        self.assertNotIn(self.base, result.stdout)

    def test_no_range_falls_back(self):
        self.assert_success(*self.invoke(self.request('/no-range')))

    def test_unknown_length_single_stream(self):
        self.assert_success(*self.invoke(self.request('/unknown-size', connections=1)))

    def test_hash_failure_does_not_replace_existing_pack(self):
        self.output.write_bytes(b'old verified pack')
        request = self.request(overwrite=True)
        request['sha256'] = '0' * 64
        result, events = self.invoke(request)
        self.assertEqual(result.returncode, 4, events)
        self.assertEqual(events[-1]['code'], 'integrity_mismatch')
        self.assertFalse(any(e['event'] == 'complete' for e in events))
        self.assertEqual(self.output.read_bytes(), b'old verified pack')

    def test_size_failure_does_not_publish(self):
        request = self.request()
        request['expected_size'] += 1
        result, events = self.invoke(request)
        self.assertEqual(result.returncode, 4, events)
        self.assertFalse(self.output.exists())

    def test_existing_file_requires_overwrite(self):
        self.output.write_bytes(b'keep')
        result, events = self.invoke(self.request())
        self.assertEqual(result.returncode, 2, events)
        self.assertEqual(self.output.read_bytes(), b'keep')

    def test_verified_overwrite(self):
        self.output.write_bytes(b'old')
        self.assert_success(*self.invoke(self.request(overwrite=True)))

    def test_expired_link_is_structured_and_redacted(self):
        result, events = self.invoke(self.request('/auth?token=DO-NOT-PRINT'))
        self.assertEqual(result.returncode, 1, events)
        self.assertEqual(events[-1]['http_status'], 403)
        self.assertTrue(events[-1]['refresh_url'])
        self.assertFalse(events[-1]['retryable'])
        self.assertNotIn('DO-NOT-PRINT', result.stdout + result.stderr)

    def test_cross_origin_redirect_stops(self):
        result, events = self.invoke(self.request('/redirect', headers={'X-Secret': 'DO-NOT-PRINT'}))
        self.assertEqual(result.returncode, 1, events)
        self.assertEqual(events[-1]['http_status'], 302)

    def test_mismatched_content_range_is_rejected(self):
        result, events = self.invoke(self.request('/bad-range', connections=4))
        self.assertEqual(result.returncode, 1, events)
        self.assertEqual(events[-1]['code'], 'invalid_response')
        self.assertFalse(self.output.exists())

    def test_download_without_expected_hash_still_reports_actual_hash(self):
        request = self.request()
        del request['sha256']
        self.assert_success(*self.invoke(request))

    def test_reserved_header_rejected(self):
        result, events = self.invoke(self.request(headers={'Range': 'bytes=10-20'}))
        self.assertEqual(result.returncode, 2, events)

    def test_output_lock_prevents_concurrent_writers(self):
        Handler.slow = True
        proc = subprocess.Popen([BINARY, '--request-stdin'], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8')
        self.addCleanup(lambda: proc.kill() if proc.poll() is None else None)
        proc.stdin.write(json.dumps(self.request('/slow')) + '\n')
        proc.stdin.flush()
        self.assertEqual(json.loads(proc.stdout.readline())['event'], 'started')
        result, events = self.invoke(self.request())
        self.assertEqual(result.returncode, 2, events)
        proc.stdin.write('{"command":"cancel"}\n')
        proc.stdin.flush()
        self.assertEqual(proc.wait(timeout=10), 3)
        proc.stdin.close()
        proc.stdout.close()
        proc.stderr.close()
        Handler.slow = False
        self.assert_success(*self.invoke(self.request()))

    def test_oversized_request_is_rejected(self):
        result = subprocess.run([BINARY, '--request-stdin'], input='x' * 70000 + '\n',
                                text=True, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(json.loads(result.stdout)['event'], 'error')

    def test_invalid_json_is_redacted(self):
        result = subprocess.run([BINARY, '--request-stdin'], input='SECRET invalid json\n',
                                text=True, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 2)
        self.assertNotIn('SECRET', result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout)['event'], 'error')

    def test_closed_or_open_stdin_does_not_hold_process_open(self):
        proc = subprocess.Popen([BINARY, '--request-stdin'], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8')
        self.addCleanup(lambda: proc.kill() if proc.poll() is None else None)
        proc.stdin.write(json.dumps(self.request()) + '\n')
        proc.stdin.flush()
        self.assertEqual(proc.wait(timeout=20), 0)
        proc.stdin.close()
        events = [json.loads(line) for line in proc.stdout]
        proc.stdout.close()
        proc.stderr.close()
        self.assertEqual(events[-1]['event'], 'complete')

    def cancel_and_resume(self, connections):
        Handler.slow = True
        request = self.request('/slow?signature=DO-NOT-STORE', connections=connections, chunk_size=65536)
        proc = subprocess.Popen([BINARY, '--request-stdin'], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8')
        self.addCleanup(lambda: proc.kill() if proc.poll() is None else None)
        proc.stdin.write(json.dumps(request) + '\n')
        proc.stdin.flush()
        received = queue.Queue()
        def read_events():
            for line in proc.stdout:
                received.put(json.loads(line))
        thread = threading.Thread(target=read_events, daemon=True)
        thread.start()
        events = []
        while True:
            event = received.get(timeout=15)
            events.append(event)
            if event['event'] == 'progress' and event['downloaded'] >= 65536:
                break
        proc.stdin.write('{"command":"cancel"}\n')
        proc.stdin.flush()
        self.assertEqual(proc.wait(timeout=10), 3, events)
        thread.join(timeout=2)
        proc.stdin.close()
        proc.stdout.close()
        proc.stderr.close()
        while not received.empty():
            events.append(received.get_nowait())
        self.assertEqual(events[-1]['event'], 'cancelled', events)
        self.assertFalse(self.output.exists())
        self.assertNotIn(b'DO-NOT-STORE', Path(str(self.output) + '.json-dl-part.dl').read_bytes())
        Handler.slow = False
        request['url'] = self.base + '/slow?signature=refreshed'
        result, events = self.invoke(request)
        self.assert_success(result, events)
        self.assertTrue(events[-1]['resumed'], events)

    def test_parallel_cancel_checkpoint_and_refreshed_url_resume(self):
        self.cancel_and_resume(4)

    def test_single_cancel_checkpoint_and_resume(self):
        self.cancel_and_resume(1)

    def test_invalid_control_cancels_with_protocol_error(self):
        result, events = self.invoke(self.request('/slow'), {'command': 'unknown'})
        self.assertEqual(result.returncode, 2, events)
        self.assertEqual(events[-1]['code'], 'invalid_request')


if __name__ == '__main__':
    unittest.main(verbosity=2)

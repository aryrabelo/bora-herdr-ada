import json
import os
import socket
import sys
import tempfile
import threading
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import bora_api  # noqa: E402


class FakeServer(object):
    """Answers each connection with the next canned reply line and records requests."""

    def __init__(self, replies):
        self.dir = tempfile.mkdtemp(prefix="ag")
        self.path = os.path.join(self.dir, "s")
        self.replies = list(replies)
        self.requests = []
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.bind(self.path)
        self.sock.listen(4)
        self.thread = threading.Thread(target=self.serve)
        self.thread.daemon = True
        self.thread.start()

    def serve(self):
        while self.replies:
            conn, _ = self.sock.accept()
            data = b""
            while b"\n" not in data:
                data += conn.recv(4096)
            request = json.loads(data.decode())
            self.requests.append(request)
            reply = self.replies.pop(0)
            reply = dict(reply, id=request["id"])
            conn.sendall((json.dumps(reply) + "\n").encode())
            conn.close()

    def close(self):
        self.thread.join(5)
        self.sock.close()
        os.unlink(self.path)
        os.rmdir(self.dir)


class SocketPathTest(unittest.TestCase):
    def test_unset_env_fails_clearly_and_never_guesses(self):
        env = {k: v for k, v in os.environ.items() if k != "HERDR_SOCKET_PATH"}
        with mock.patch.dict(os.environ, env, clear=True):
            with self.assertRaises(bora_api.ApiError) as ctx:
                bora_api.call("workspace.list")
        self.assertEqual(ctx.exception.code, "no_socket")
        self.assertIn("HERDR_SOCKET_PATH", str(ctx.exception))

    def test_blank_env_counts_as_unset(self):
        with mock.patch.dict(os.environ, {"HERDR_SOCKET_PATH": "  "}):
            with self.assertRaises(bora_api.ApiError):
                bora_api.socket_path()


class CallTest(unittest.TestCase):
    def test_result_and_request_shape(self):
        server = FakeServer([{"result": {"type": "workspace_list", "workspaces": [{"workspace_id": "w1"}]}}])
        try:
            with mock.patch.dict(os.environ, {"HERDR_SOCKET_PATH": server.path}):
                self.assertEqual(bora_api.workspace_list(), [{"workspace_id": "w1"}])
        finally:
            server.close()
        self.assertEqual(server.requests[0]["method"], "workspace.list")

    def test_set_group_params_and_error_mapping(self):
        server = FakeServer(
            [
                {"result": {"type": "workspace_info", "workspace": {}}},
                {"error": {"code": "workspace_not_found", "message": "workspace w9 not found"}},
            ]
        )
        try:
            with mock.patch.dict(os.environ, {"HERDR_SOCKET_PATH": server.path}):
                bora_api.workspace_set_group("w1", "pp/docs")
                with self.assertRaises(bora_api.ApiError) as ctx:
                    bora_api.workspace_get("w9")
        finally:
            server.close()
        self.assertEqual(server.requests[0]["params"], {"workspace_id": "w1", "group": "pp/docs"})
        self.assertEqual(ctx.exception.code, "workspace_not_found")

    def test_unreachable_socket_is_an_api_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.dict(os.environ, {"HERDR_SOCKET_PATH": os.path.join(tmp, "nope")}):
                with self.assertRaises(bora_api.ApiError) as ctx:
                    bora_api.call("ping")
        self.assertEqual(ctx.exception.code, "connect_failed")


if __name__ == "__main__":
    unittest.main()

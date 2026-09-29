"""Minimal NDJSON client for the bora socket API (Python 3.9, stdlib only).

Speaks one request per connection: write `{"id","method","params"}\\n`, read one
line back. The socket path comes ONLY from HERDR_SOCKET_PATH; there is no
default and no guessing, so a manual or test run can never reach a server the
caller did not name explicitly.
"""

import itertools
import json
import os
import socket

SOCKET_ENV = "HERDR_SOCKET_PATH"

_ids = itertools.count(1)


class ApiError(Exception):
    """A failed call. `code` is the API error code, or a local marker."""

    def __init__(self, code, message):
        Exception.__init__(self, "%s: %s" % (code, message))
        self.code = code
        self.message = message


def socket_path():
    path = os.environ.get(SOCKET_ENV, "").strip()
    if not path:
        raise ApiError(
            "no_socket",
            "%s is not set; run this from inside bora (plugin hook/action, or a "
            "bora pane), or export %s to the socket of the server to talk to"
            % (SOCKET_ENV, SOCKET_ENV),
        )
    return path


def call(method, params=None, timeout=15.0):
    """Send one request, return its `result` object, raise ApiError on failure."""
    path = socket_path()
    request = {"id": "ag-%d-%d" % (os.getpid(), next(_ids)), "method": method, "params": params or {}}
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    try:
        try:
            sock.connect(path)
        except (OSError, socket.timeout) as exc:
            raise ApiError("connect_failed", "cannot connect to %s: %s" % (path, exc))
        sock.sendall((json.dumps(request) + "\n").encode("utf-8"))
        buf = b""
        while b"\n" not in buf:
            try:
                chunk = sock.recv(65536)
            except (OSError, socket.timeout) as exc:
                raise ApiError("read_failed", "%s: %s" % (method, exc))
            if not chunk:
                break
            buf += chunk
    finally:
        sock.close()
    line = buf.split(b"\n", 1)[0]
    if not line.strip():
        raise ApiError("empty_response", "%s: server closed the connection without a reply" % method)
    try:
        reply = json.loads(line.decode("utf-8"))
    except ValueError as exc:
        raise ApiError("bad_response", "%s: unparseable reply: %s" % (method, exc))
    error = reply.get("error")
    if error:
        raise ApiError(error.get("code", "error"), error.get("message", ""))
    return reply.get("result") or {}


def workspace_list():
    return call("workspace.list").get("workspaces", [])


def workspace_get(workspace_id):
    return call("workspace.get", {"workspace_id": workspace_id}).get("workspace", {})


def workspace_set_group(workspace_id, group):
    return call("workspace.set_group", {"workspace_id": workspace_id, "group": group})


def pane_list(workspace_id):
    return call("pane.list", {"workspace_id": workspace_id}).get("panes", [])

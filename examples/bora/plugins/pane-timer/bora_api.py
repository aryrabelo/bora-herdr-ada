"""Minimal NDJSON client for the bora socket API (stdlib only, Python 3.9+).

The socket path is read ONLY from HERDR_SOCKET_PATH. There is deliberately no
default or guessed path: a manual or test run without the variable must fail
loudly instead of reaching whichever server happens to be running.
"""

import json
import os
import socket
import itertools

SOCKET_ENV = "HERDR_SOCKET_PATH"
MAX_RESPONSE_BYTES = 8 * 1024 * 1024

_request_ids = itertools.count(1)


class SocketPathNotSet(Exception):
    """HERDR_SOCKET_PATH is missing or empty; nothing to talk to."""


class SocketUnreachable(Exception):
    """The socket exists in the environment but the server did not answer."""


class ApiError(Exception):
    """The server answered with an error object."""

    def __init__(self, code, message):
        super().__init__("%s: %s" % (code, message))
        self.code = code
        self.message = message


def socket_path(env=None):
    env = os.environ if env is None else env
    path = env.get(SOCKET_ENV, "")
    if not path:
        raise SocketPathNotSet(
            "%s is not set; run this from inside bora (a pane, plugin action or "
            "popup) or export %s to the server socket you mean to use" % (SOCKET_ENV, SOCKET_ENV)
        )
    return path


def call(method, params=None, env=None, timeout=5.0):
    """Send one request, return the `result` object. Raises ApiError on an error reply."""
    path = socket_path(env)
    request_id = "pane-timer-%d-%d" % (os.getpid(), next(_request_ids))
    payload = (json.dumps({"id": request_id, "method": method, "params": params or {}}) + "\n").encode("utf-8")
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    try:
        try:
            sock.connect(path)
            sock.sendall(payload)
            buf = b""
            while not buf.endswith(b"\n"):
                chunk = sock.recv(65536)
                if not chunk:
                    break
                buf += chunk
                if len(buf) > MAX_RESPONSE_BYTES:
                    raise SocketUnreachable("response from %s exceeded %d bytes" % (path, MAX_RESPONSE_BYTES))
        except (OSError, socket.timeout) as exc:
            raise SocketUnreachable("cannot reach bora socket %s: %s" % (path, exc))
    finally:
        sock.close()
    if not buf.strip():
        raise SocketUnreachable("bora socket %s closed without a reply" % path)
    try:
        reply = json.loads(buf.decode("utf-8"))
    except ValueError as exc:
        raise SocketUnreachable("unparseable reply from %s: %s" % (path, exc))
    if "error" in reply:
        error = reply["error"] or {}
        raise ApiError(error.get("code", "unknown"), error.get("message", ""))
    return reply.get("result", {})


def ping(env=None, timeout=3.0):
    return call("ping", {}, env=env, timeout=timeout)


def pane_get(pane_id, env=None):
    return call("pane.get", {"pane_id": pane_id}, env=env)["pane"]


def pane_set_status(pane_id, status, env=None):
    """Pin `status` on the pane. `status=None` clears the pin ("Auto")."""
    params = {"pane_id": pane_id}
    if status is not None:
        params["status"] = status
    return call("pane.set_status", params, env=env)


def plugin_pane_open(plugin_id, entrypoint, extra_env=None, env=None):
    params = {"plugin_id": plugin_id, "entrypoint": entrypoint}
    if extra_env:
        params["env"] = dict(extra_env)
    return call("plugin.pane.open", params, env=env)

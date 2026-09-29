#!/usr/bin/env python3
"""ary.pane-timer: flip a pane's sidebar status to blocked/done after a delay.

One file, three roles (see README.md):
  * CLI      add / list / cancel / stop
  * daemon   started by the plugin's [[startup]] hook; fires due timers
  * prompt   popup UI opened by the `set` action

Python 3.9 compatible, stdlib only. Talks to bora through bora_api.py, which
only ever uses the socket named by HERDR_SOCKET_PATH.
"""

import argparse
import contextlib
import fcntl
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time

import bora_api

PLUGIN_ID = "ary.pane-timer"
STATUSES = ("idle", "working", "blocked", "done")
STATUS_ALIASES = {"red": "blocked"}
MAX_DELAY_SECONDS = 30 * 24 * 3600
TICK_SECONDS = 1.0
PROBE_INTERVAL_SECONDS = 2.0
DEFAULT_SOCKET_GRACE_SECONDS = 60.0
PANE_GONE_CODE = "pane_not_found"
TARGET_ENV = "PANE_TIMER_TARGET"
STOP_WAIT_SECONDS = 5.0

_DURATION_RE = re.compile(r"^(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?$", re.ASCII)
_BARE_INT_RE = re.compile(r"^\d+$", re.ASCII)


class CommandError(Exception):
    """A user-facing failure: printed without a traceback, exit status 1."""


class StateError(Exception):
    """The timer state file exists but cannot be read."""


# --------------------------------------------------------------------------
# Parsing and formatting
# --------------------------------------------------------------------------


def parse_duration(text):
    """`90s`, `10m`, `1h30m` (combinable, h before m before s); a bare integer is minutes."""
    raw = "" if text is None else str(text).strip().lower()
    if not raw:
        raise ValueError("delay is empty; use e.g. 90s, 10m, 1h30m (bare number = minutes)")
    if _BARE_INT_RE.match(raw):
        seconds = int(raw) * 60
    else:
        match = _DURATION_RE.match(raw)
        if not match or not any(match.groups()):
            raise ValueError(
                "invalid delay %r; use e.g. 90s, 10m, 1h30m (bare number = minutes)" % str(text).strip()
            )
        hours, minutes, secs = (int(g) if g else 0 for g in match.groups())
        seconds = hours * 3600 + minutes * 60 + secs
    if seconds <= 0:
        raise ValueError("delay must be greater than zero")
    if seconds > MAX_DELAY_SECONDS:
        raise ValueError("delay %r is longer than the 30 day maximum" % str(text).strip())
    return seconds


def parse_status(text):
    """A status `pane.set_status` accepts, or the alias `red` for `blocked`."""
    raw = "" if text is None else str(text).strip().lower()
    raw = STATUS_ALIASES.get(raw, raw)
    if raw not in STATUSES:
        raise ValueError(
            "invalid status %r; use one of %s (or red = blocked)" % (str(text).strip(), "|".join(STATUSES))
        )
    return raw


def format_duration(seconds):
    seconds = max(0, int(round(seconds)))
    hours, rest = divmod(seconds, 3600)
    minutes, secs = divmod(rest, 60)
    parts = []
    if hours:
        parts.append("%dh" % hours)
    if minutes:
        parts.append("%dm" % minutes)
    if secs or not parts:
        parts.append("%ds" % secs)
    return "".join(parts)


def format_clock(ts, now):
    """Wall-clock time of `ts`; the date is added when it is not today."""
    local = time.localtime(ts)
    if time.strftime("%Y-%m-%d", local) == time.strftime("%Y-%m-%d", time.localtime(now)):
        return time.strftime("%H:%M:%S", local)
    return time.strftime("%Y-%m-%d %H:%M:%S", local)


def log(message):
    line = "%s pane-timer[%d] %s" % (time.strftime("%Y-%m-%dT%H:%M:%S%z"), os.getpid(), message)
    try:
        print(line, flush=True)
    except OSError:
        pass  # stdout is gone (server stopped); the daemon keeps working


# --------------------------------------------------------------------------
# State directory, timer store, daemon lock
# --------------------------------------------------------------------------


def state_dir(env=None):
    """HERDR_PLUGIN_STATE_DIR when bora provided it, else the path bora would have used.

    The fallback mirrors `state_dir()/plugins/<id>` in bora's src/config/io.rs and
    src/plugin_paths.rs: `$XDG_STATE_HOME/<ns>` or `$HOME/.local/state/<ns>`, where
    <ns> is HERDR_NAMESPACE or `bora`. The plugin id is already a valid path
    component (lowercase, dot, hyphen), so bora does not escape it.
    """
    env = os.environ if env is None else env
    provided = env.get("HERDR_PLUGIN_STATE_DIR")
    if provided:
        return provided
    namespace = (env.get("HERDR_NAMESPACE") or "").strip() or "bora"
    xdg = env.get("XDG_STATE_HOME")
    if xdg:
        base = os.path.join(xdg, namespace)
    else:
        home = env.get("HOME") or os.path.expanduser("~")
        base = os.path.join(home, ".local", "state", namespace)
    return os.path.join(base, "plugins", PLUGIN_ID)


def due_timers(timers, now):
    """Timers whose deadline is at or before `now`, earliest first."""
    return sorted((t for t in timers.values() if t["due"] <= now), key=lambda t: (t["due"], t["pane_id"]))


def socket_grace_exceeded(unreachable_since, now, grace_seconds):
    """True once the socket has been unreachable, without a success, for the grace period."""
    return unreachable_since is not None and now - unreachable_since >= grace_seconds


def _valid_timer(entry):
    return (
        isinstance(entry, dict)
        and isinstance(entry.get("pane_id"), str)
        and entry.get("status") in STATUSES
        and isinstance(entry.get("due"), (int, float))
    )


class Store:
    """Timers in one JSON file: reads are lock-free (atomic replace), writes take a flock."""

    def __init__(self, directory):
        self.directory = str(directory)
        self.path = os.path.join(self.directory, "timers.json")
        self.lock_path = os.path.join(self.directory, "timers.lock")

    @contextlib.contextmanager
    def locked(self):
        os.makedirs(self.directory, exist_ok=True)
        fd = os.open(self.lock_path, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX)
            yield
        finally:
            os.close(fd)  # closing releases the flock

    def read(self):
        try:
            with open(self.path, "r", encoding="utf-8") as handle:
                data = json.load(handle)
        except FileNotFoundError:
            return {}
        except (OSError, ValueError) as exc:
            raise StateError("cannot read %s: %s (delete it to reset all timers)" % (self.path, exc))
        entries = data.get("timers") if isinstance(data, dict) else None
        if not isinstance(entries, dict):
            raise StateError("%s has no timers object (delete it to reset all timers)" % self.path)
        return {pane: dict(entry) for pane, entry in entries.items() if _valid_timer(entry)}

    def _write(self, timers):
        tmp = "%s.%d.tmp" % (self.path, os.getpid())
        payload = json.dumps({"version": 1, "timers": timers}, indent=2, sort_keys=True)
        with open(tmp, "w", encoding="utf-8") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp, self.path)

    def add(self, pane_id, status, delay_seconds, now=None):
        """Arm `pane_id`, replacing any timer already on it. Returns (timer, previous)."""
        now = time.time() if now is None else now
        timer = {
            "pane_id": pane_id,
            "status": status,
            "due": now + delay_seconds,
            "created": now,
            "delay_seconds": delay_seconds,
        }
        with self.locked():
            timers = self.read()
            previous = timers.get(pane_id)
            timers[pane_id] = timer
            self._write(timers)
        return timer, previous

    def cancel(self, pane_id):
        with self.locked():
            timers = self.read()
            removed = timers.pop(pane_id, None)
            if removed is not None:
                self._write(timers)
        return removed

    def cancel_all(self):
        with self.locked():
            timers = self.read()
            if timers:
                self._write({})
        return sorted(timers.values(), key=lambda t: (t["due"], t["pane_id"]))

    def remove_if_same(self, timer):
        """Drop `timer` unless it was replaced or cancelled since it was read."""
        with self.locked():
            timers = self.read()
            current = timers.get(timer["pane_id"])
            if current is None or current["due"] != timer["due"] or current["status"] != timer["status"]:
                return False
            del timers[timer["pane_id"]]
            self._write(timers)
        return True


class DaemonLock:
    """Single-instance guard: an exclusive non-blocking flock; the holder records its pid."""

    def __init__(self, directory):
        self.directory = str(directory)
        self.path = os.path.join(self.directory, "daemon.lock")
        self._fd = None

    def acquire(self):
        os.makedirs(self.directory, exist_ok=True)
        fd = os.open(self.path, os.O_RDWR | os.O_CREAT, 0o600)  # no O_TRUNC: losers must not wipe the pid
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            os.close(fd)
            return False
        os.ftruncate(fd, 0)
        os.write(fd, ("%d\n" % os.getpid()).encode("ascii"))
        self._fd = fd
        return True

    def release(self):
        if self._fd is not None:
            os.close(self._fd)
            self._fd = None

    @staticmethod
    def is_held(directory):
        path = os.path.join(str(directory), "daemon.lock")
        try:
            fd = os.open(path, os.O_RDWR)
        except OSError:
            return False
        try:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError:
                return True
            return False
        finally:
            os.close(fd)

    @staticmethod
    def holder_pid(directory):
        try:
            with open(os.path.join(str(directory), "daemon.lock"), "r") as handle:
                return int(handle.read().strip())
        except (OSError, ValueError):
            return None


# --------------------------------------------------------------------------
# Arming (shared by `add` and the popup prompt)
# --------------------------------------------------------------------------


def arm_timer(pane_id, delay_seconds, status, env=None, now=None):
    """Validate the pane through the API, then arm. Returns (timer, previous). Does not start the daemon."""
    env = os.environ if env is None else env
    try:
        bora_api.pane_get(pane_id, env=env)
    except bora_api.ApiError as exc:
        if exc.code == PANE_GONE_CODE:
            raise CommandError("pane %s does not exist" % pane_id)
        raise CommandError("bora refused the pane lookup: %s" % exc)
    except (bora_api.SocketPathNotSet, bora_api.SocketUnreachable) as exc:
        raise CommandError(str(exc))
    directory = state_dir(env)
    try:
        timer, previous = Store(directory).add(pane_id, status, delay_seconds, now=now)
    except StateError as exc:
        raise CommandError(str(exc))
    return timer, previous


def describe_armed(timer, previous, now):
    text = "armed %s: %s at %s (in %s)" % (
        timer["pane_id"],
        timer["status"],
        format_clock(timer["due"], now),
        format_duration(timer["due"] - now),
    )
    if previous is not None:
        text += "; replaced previous timer (%s, was due %s)" % (previous["status"], format_clock(previous["due"], now))
    return text


DAEMON_START_WAIT_SECONDS = 2.0
DAEMON_LOG_MAX_BYTES = 512 * 1024


def spawn_daemon(env, directory):
    """Start a detached daemon; its stdout/stderr are appended to <state dir>/daemon.log."""
    os.makedirs(directory, exist_ok=True)
    log_path = os.path.join(directory, "daemon.log")
    mode = "ab"
    try:
        if os.path.getsize(log_path) > DAEMON_LOG_MAX_BYTES:
            mode = "wb"
    except OSError:
        pass
    script = os.path.abspath(__file__)
    with open(log_path, mode) as out:
        return subprocess.Popen(
            [sys.executable, script, "daemon"],
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=out,
            env=dict(env),
            cwd=os.path.dirname(script),
            start_new_session=True,
            close_fds=True,
        )


def ensure_daemon(env=None, spawner=None, wait_seconds=DAEMON_START_WAIT_SECONDS, sleep=time.sleep, clock=time.monotonic):
    """Make sure a daemon holds the lock. Returns 'running' or 'started'; raises CommandError.

    `[[startup]]` does not run on `plugin link`, so the first timer after linking has to
    start the daemon itself. A racing startup daemon is harmless: the flock lets one win.
    """
    env = os.environ if env is None else env
    directory = state_dir(env)
    if DaemonLock.is_held(directory):
        return "running"
    log_path = os.path.join(directory, "daemon.log")
    try:
        child = (spawner or spawn_daemon)(env, directory)
    except OSError as exc:
        raise CommandError("could not start the timer daemon: %s (log: %s)" % (exc, log_path))
    deadline = clock() + wait_seconds
    while True:
        if DaemonLock.is_held(directory):
            return "started"
        exit_status = child.poll() if child is not None else None
        if exit_status is not None:
            raise CommandError("the timer daemon exited with status %s without taking its lock; see %s" % (exit_status, log_path))
        if clock() >= deadline:
            break
        sleep(0.05)
    raise CommandError("the timer daemon did not take its lock within %gs; see %s" % (wait_seconds, log_path))


# --------------------------------------------------------------------------
# Daemon
# --------------------------------------------------------------------------


class BoraSocketApi:
    """The daemon's view of bora; swapped for a fake in tests."""

    def __init__(self, env=None):
        self.env = env

    def set_status(self, pane_id, status):
        return bora_api.pane_set_status(pane_id, status, env=self.env)

    def ping(self):
        return bora_api.ping(env=self.env)


class Daemon:
    def __init__(self, store, api, grace_seconds, log_fn=log, probe_interval=PROBE_INTERVAL_SECONDS):
        self.store = store
        self.api = api
        self.grace_seconds = grace_seconds
        self.log = log_fn
        self.probe_interval = probe_interval
        self.unreachable_since = None
        self._last_contact = None
        self._reported = set()

    def _reachable(self, now):
        if self.unreachable_since is not None:
            self.log("socket reachable again after %ds" % round(now - self.unreachable_since))
        self.unreachable_since = None
        self._last_contact = now

    def _unreachable(self, now, exc):
        if self.unreachable_since is None:
            self.unreachable_since = now
            self.log("socket unreachable (%s); exiting if this lasts %gs" % (exc, self.grace_seconds))
        self._last_contact = now

    def tick(self, now):
        """One pass. Returns True when the daemon should exit (socket grace exceeded)."""
        try:
            timers = self.store.read()
        except StateError as exc:
            if str(exc) not in self._reported:
                self._reported.add(str(exc))
                self.log("state error: %s" % exc)
            timers = {}
        for timer in due_timers(timers, now):
            if not self._fire(timer, now):
                break
        # Probe when idle (so a dead server is noticed without timers), and every tick
        # while the socket is down so recovery/grace are judged promptly.
        stale = self._last_contact is None or now - self._last_contact >= self.probe_interval
        if self._last_contact != now and (stale or self.unreachable_since is not None):
            try:
                self.api.ping()
                self._reachable(now)
            except bora_api.SocketUnreachable as exc:
                self._unreachable(now, exc)
            except bora_api.ApiError:
                self._reachable(now)  # the server answered, so it is up
        if socket_grace_exceeded(self.unreachable_since, now, self.grace_seconds):
            self.log("socket unreachable for %gs; exiting (the next bora server start respawns the daemon)" % self.grace_seconds)
            return True
        return False

    def _fire(self, timer, now):
        """Fire one timer. Returns False when the socket is down (stop trying this tick)."""
        pane, status = timer["pane_id"], timer["status"]
        try:
            self.api.set_status(pane, status)
        except bora_api.SocketUnreachable as exc:
            self._unreachable(now, exc)
            return False
        except bora_api.ApiError as exc:
            self._reachable(now)
            if exc.code == PANE_GONE_CODE:
                self.store.remove_if_same(timer)
                self.log("pane %s no longer exists; dropped its %s timer" % (pane, status))
            else:
                key = (pane, timer["due"], exc.code)
                if key not in self._reported:
                    self._reported.add(key)
                    self.log("set_status(%s, %s) failed: %s; will retry each tick" % (pane, status, exc))
            return True
        self._reachable(now)
        if self.store.remove_if_same(timer):
            self.log("fired: %s -> %s (was due %s)" % (pane, status, format_clock(timer["due"], now)))
        else:
            self.log("fired: %s -> %s; timer was replaced or cancelled meanwhile, newer state kept" % (pane, status))
        return True


def grace_from_env(env):
    raw = env.get("PANE_TIMER_SOCKET_GRACE_SECONDS")
    if raw is None or raw.strip() == "":
        return DEFAULT_SOCKET_GRACE_SECONDS
    try:
        value = float(raw)
        if value < 0:
            raise ValueError("negative")
        return value
    except ValueError:
        log("ignoring invalid PANE_TIMER_SOCKET_GRACE_SECONDS=%r; using %g" % (raw, DEFAULT_SOCKET_GRACE_SECONDS))
        return DEFAULT_SOCKET_GRACE_SECONDS


def run_daemon(env=None):
    env = os.environ if env is None else env
    try:
        bora_api.socket_path(env)
    except bora_api.SocketPathNotSet as exc:
        log("cannot start: %s" % exc)
        return 2
    directory = state_dir(env)
    lock = DaemonLock(directory)
    if not lock.acquire():
        log("another daemon already holds %s; exiting" % lock.path)
        return 0
    stop = threading.Event()

    def on_signal(signum, _frame):
        log("received signal %d; stopping" % signum)
        stop.set()

    signal.signal(signal.SIGTERM, on_signal)
    signal.signal(signal.SIGINT, on_signal)
    store = Store(directory)
    grace = grace_from_env(env)
    try:
        pending = len(store.read())
    except StateError:
        pending = 0
    log("daemon started; state dir %s; %d timer(s) pending; socket grace %gs" % (directory, pending, grace))
    daemon = Daemon(store, BoraSocketApi(env), grace)
    try:
        while not stop.is_set():
            if daemon.tick(time.time()):
                break
            stop.wait(TICK_SECONDS)
    finally:
        lock.release()
    log("daemon exited")
    return 0


# --------------------------------------------------------------------------
# Target resolution, popup prompt
# --------------------------------------------------------------------------


def focused_pane(env):
    """Pane the invocation targets: explicit override, then plugin context, then HERDR_PANE_ID."""
    explicit = env.get(TARGET_ENV)
    if explicit:
        return explicit
    context = env.get("HERDR_PLUGIN_CONTEXT_JSON")
    if context:
        try:
            pane = json.loads(context).get("focused_pane_id")
            if pane:
                return pane
        except (ValueError, AttributeError):
            pass
    return env.get("HERDR_PANE_ID") or None


def read_status_choice(text):
    """Prompt answer: empty/`b` -> blocked, `d` -> done, otherwise any accepted status word."""
    raw = text.strip().lower()
    if raw in ("", "b"):
        return "blocked"
    if raw == "d":
        return "done"
    return parse_status(raw)


def run_prompt(env=None, input_fn=input, out=None, hold_seconds=1.2, sleep=time.sleep, ensure=None):
    env = os.environ if env is None else env
    out = sys.stdout if out is None else out

    def say(text):
        print(text, file=out, flush=True)

    pane_id = focused_pane(env)
    if not pane_id:
        say("pane-timer: cannot tell which pane to arm (no focused pane in the plugin context)")
        sleep(3)
        return 1
    try:
        existing = Store(state_dir(env)).read().get(pane_id)
    except StateError:
        existing = None
    now = time.time()
    say("pane-timer for %s" % pane_id)
    if existing is not None:
        say("current timer: %s at %s (in %s)" % (
            existing["status"], format_clock(existing["due"], now), format_duration(existing["due"] - now)))
    try:
        while True:
            answer = input_fn("Delay (10m, 1h30m; bare number = minutes; empty = cancel): ")
            if not answer.strip():
                say("cancelled, nothing changed")
                sleep(hold_seconds)
                return 0
            try:
                delay = parse_duration(answer)
                break
            except ValueError as exc:
                say("  %s" % exc)
        while True:
            answer = input_fn("Status [b]locked (red) / [d]one (default b): ")
            try:
                status = read_status_choice(answer)
                break
            except ValueError as exc:
                say("  %s" % exc)
    except (EOFError, KeyboardInterrupt):
        say("cancelled, nothing changed")
        sleep(hold_seconds)
        return 0
    try:
        timer, previous = arm_timer(pane_id, delay, status, env=env)
    except CommandError as exc:
        say("pane-timer: %s" % exc)
        sleep(3)
        return 1
    now = time.time()
    say(describe_armed(timer, previous, now))
    try:
        state = (ensure or ensure_daemon)(env)
    except CommandError as exc:
        say("pane-timer: %s" % exc)
        sleep(6)
        return 1
    if state == "started":
        say("started the timer daemon")
    sleep(hold_seconds)
    return 0


def open_prompt(env=None):
    """Action `set`: open the popup, telling it which pane the action was invoked on."""
    env = os.environ if env is None else env
    pane_id = focused_pane(env)
    if not pane_id:
        raise CommandError("no focused pane in the plugin context; invoke this from a pane")
    try:
        bora_api.plugin_pane_open(PLUGIN_ID, "prompt", extra_env={TARGET_ENV: pane_id}, env=env)
    except bora_api.ApiError as exc:
        raise CommandError("could not open the timer popup: %s" % exc)
    except (bora_api.SocketPathNotSet, bora_api.SocketUnreachable) as exc:
        raise CommandError(str(exc))
    print("opened timer popup for %s" % pane_id)
    return 0


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def cmd_add(args, env):
    try:
        delay = parse_duration(args.delay)
        status = parse_status(args.status)
    except ValueError as exc:
        raise CommandError(str(exc))
    timer, previous = arm_timer(args.pane_id, delay, status, env=env)
    print(describe_armed(timer, previous, time.time()))
    try:
        state = ensure_daemon(env)
    except CommandError as exc:
        raise CommandError("timer armed on %s, but %s" % (timer["pane_id"], exc))
    if state == "started":
        print("started the timer daemon (log: %s)" % os.path.join(state_dir(env), "daemon.log"))
    return 0


def cmd_list(args, env):
    try:
        timers = list(Store(state_dir(env)).read().values())
    except StateError as exc:
        raise CommandError(str(exc))
    timers.sort(key=lambda t: (t["due"], t["pane_id"]))
    now = time.time()
    if args.json:
        print(json.dumps([
            {
                "pane_id": t["pane_id"],
                "status": t["status"],
                "due_unix": t["due"],
                "due_at": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(t["due"])),
                "due_in_seconds": round(t["due"] - now, 3),
            }
            for t in timers
        ], indent=2))
        return 0
    if not timers:
        print("no timers")
    else:
        print("%-12s %-8s %-10s %s" % ("PANE", "STATUS", "DUE IN", "DUE AT"))
        for t in timers:
            remaining = t["due"] - now
            due_in = "overdue" if remaining <= 0 else format_duration(remaining)
            print("%-12s %-8s %-10s %s" % (t["pane_id"], t["status"], due_in, format_clock(t["due"], now)))
    directory = state_dir(env)
    if DaemonLock.is_held(directory):
        print("daemon: running (pid %s)" % (DaemonLock.holder_pid(directory) or "?"))
    else:
        print("daemon: NOT running (the next `add` or bora server start starts it)")
    return 0


def cmd_cancel(args, env):
    store = Store(state_dir(env))
    try:
        if args.all:
            removed = store.cancel_all()
            print("cancelled %d timer(s)" % len(removed))
            return 0
        pane_id = args.pane_id or (focused_pane(env) if args.focused else None)
        if not pane_id:
            raise CommandError("give a <pane_id>, --focused, or --all")
        removed = store.cancel(pane_id)
    except StateError as exc:
        raise CommandError(str(exc))
    if removed is None:
        print("no timer on %s" % pane_id)
    else:
        print("cancelled %s (%s, was due %s)" % (pane_id, removed["status"], format_clock(removed["due"], time.time())))
    return 0


def cmd_stop(_args, env):
    directory = state_dir(env)
    if not DaemonLock.is_held(directory):
        print("no daemon running")
        return 0
    pid = DaemonLock.holder_pid(directory)
    if not pid:
        raise CommandError("daemon lock is held but %s has no pid" % os.path.join(directory, "daemon.lock"))
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError as exc:
        raise CommandError("cannot signal daemon pid %d: %s" % (pid, exc))
    deadline = time.time() + STOP_WAIT_SECONDS
    while time.time() < deadline:
        if not DaemonLock.is_held(directory):
            print("stopped daemon (pid %d)" % pid)
            return 0
        time.sleep(0.05)
    raise CommandError("sent SIGTERM to daemon pid %d but it is still running after %gs" % (pid, STOP_WAIT_SECONDS))


def build_parser():
    parser = argparse.ArgumentParser(prog="timer.py", description="Flip a pane's sidebar status after a delay.")
    sub = parser.add_subparsers(dest="command", metavar="<cmd>")
    sub.required = True

    add = sub.add_parser("add", help="arm (or replace) the timer on a pane")
    add.add_argument("pane_id")
    add.add_argument("delay", help="90s, 10m, 1h30m; a bare number means minutes")
    add.add_argument("status", help="blocked (alias: red), done, idle, or working")
    add.set_defaults(func=cmd_add)

    lst = sub.add_parser("list", help="show armed timers")
    lst.add_argument("--json", action="store_true")
    lst.set_defaults(func=cmd_list)

    cancel = sub.add_parser("cancel", help="cancel a pane's timer (never touches an already-fired pin)")
    cancel.add_argument("pane_id", nargs="?")
    cancel.add_argument("--all", action="store_true")
    cancel.add_argument("--focused", action="store_true", help="the focused pane of the plugin context")
    cancel.set_defaults(func=cmd_cancel)

    stop = sub.add_parser("stop", help="stop the running daemon")
    stop.set_defaults(func=cmd_stop)

    daemon = sub.add_parser("daemon", help="run the timer daemon (single instance)")
    daemon.set_defaults(func=lambda _a, e: run_daemon(e))

    prompt = sub.add_parser("prompt", help="interactive prompt (popup pane)")
    prompt.set_defaults(func=lambda _a, e: run_prompt(e))

    open_cmd = sub.add_parser("open-prompt", help="open the popup for the focused pane (the `set` action)")
    open_cmd.set_defaults(func=lambda _a, e: open_prompt(e))
    return parser


def main(argv=None, env=None):
    env = os.environ if env is None else env
    args = build_parser().parse_args(argv)
    try:
        return args.func(args, env)
    except CommandError as exc:
        print("pane-timer: %s" % exc, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())

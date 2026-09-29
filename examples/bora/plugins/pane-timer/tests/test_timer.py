import contextlib
import io
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

PLUGIN_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, PLUGIN_DIR)

import bora_api  # noqa: E402
import timer  # noqa: E402


class DurationGrammar(unittest.TestCase):
    def test_valid_forms(self):
        cases = {
            "90s": 90,
            "10m": 600,
            "1h": 3600,
            "1h30m": 5400,
            "1h30m15s": 5415,
            "2m5s": 125,
            "45": 45 * 60,  # bare integer means minutes
            "  10M ": 600,  # case and surrounding space are forgiven
            "0h5m": 300,
        }
        for text, seconds in cases.items():
            with self.subTest(text=text):
                self.assertEqual(timer.parse_duration(text), seconds)

    def test_rejected_forms(self):
        for text in ["", "   ", "0", "0s", "0m", "0h0m0s", "-5", "-5m", "abc", "10x", "1m30",
                     "30m1h", "1.5h", "1h 30m", "m", "h30m", "+5", "10m10m", "١٠"]:
            with self.subTest(text=text):
                with self.assertRaises(ValueError):
                    timer.parse_duration(text)

    def test_zero_message_is_specific(self):
        with self.assertRaisesRegex(ValueError, "greater than zero"):
            timer.parse_duration("0m")

    def test_upper_bound(self):
        self.assertEqual(timer.parse_duration("720h"), 720 * 3600)
        with self.assertRaises(ValueError):
            timer.parse_duration("721h")

    def test_format_duration(self):
        self.assertEqual(timer.format_duration(5400), "1h30m")
        self.assertEqual(timer.format_duration(59.6), "1m")
        self.assertEqual(timer.format_duration(0), "0s")
        self.assertEqual(timer.format_duration(3601), "1h1s")


class StatusParsing(unittest.TestCase):
    def test_every_settable_status_and_alias(self):
        for status in ("idle", "working", "blocked", "done"):
            self.assertEqual(timer.parse_status(status), status)
        self.assertEqual(timer.parse_status("red"), "blocked")
        self.assertEqual(timer.parse_status(" RED "), "blocked")
        self.assertEqual(timer.parse_status("Done"), "done")

    def test_rejects_what_the_api_rejects_or_does_not_know(self):
        for text in ["unknown", "auto", "", "green", "b"]:
            with self.subTest(text=text):
                with self.assertRaises(ValueError):
                    timer.parse_status(text)

    def test_prompt_letters(self):
        self.assertEqual(timer.read_status_choice(""), "blocked")
        self.assertEqual(timer.read_status_choice("b"), "blocked")
        self.assertEqual(timer.read_status_choice("D"), "done")
        self.assertEqual(timer.read_status_choice("red"), "blocked")
        with self.assertRaises(ValueError):
            timer.read_status_choice("x")


class StateDirRule(unittest.TestCase):
    def test_bora_provided_dir_wins(self):
        env = {"HERDR_PLUGIN_STATE_DIR": "/s/here", "XDG_STATE_HOME": "/x", "HERDR_NAMESPACE": "n"}
        self.assertEqual(timer.state_dir(env), "/s/here")

    def test_fallback_mirrors_bora_layout(self):
        self.assertEqual(
            timer.state_dir({"XDG_STATE_HOME": "/x", "HERDR_NAMESPACE": "ns"}),
            "/x/ns/plugins/ary.pane-timer",
        )
        self.assertEqual(
            timer.state_dir({"HOME": "/h"}),
            "/h/.local/state/bora/plugins/ary.pane-timer",
        )
        self.assertEqual(
            timer.state_dir({"HOME": "/h", "HERDR_NAMESPACE": "  "}),
            "/h/.local/state/bora/plugins/ary.pane-timer",
        )


class StoreCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.dir = os.path.join(self._tmp.name, "state")
        self.store = timer.Store(self.dir)


class StoreBehavior(StoreCase):
    def test_add_persists_and_round_trips_through_a_new_store(self):
        timer_, previous = self.store.add("w1:p1", "blocked", 600, now=1000.0)
        self.assertIsNone(previous)
        self.assertEqual(timer_["due"], 1600.0)
        reread = timer.Store(self.dir).read()
        self.assertEqual(list(reread), ["w1:p1"])
        self.assertEqual(reread["w1:p1"]["status"], "blocked")
        self.assertEqual(reread["w1:p1"]["due"], 1600.0)

    def test_add_replaces_and_reports_previous(self):
        self.store.add("w1:p1", "blocked", 600, now=1000.0)
        new, previous = self.store.add("w1:p1", "done", 30, now=1100.0)
        self.assertEqual(previous["status"], "blocked")
        self.assertEqual(previous["due"], 1600.0)
        state = self.store.read()
        self.assertEqual(len(state), 1)
        self.assertEqual((state["w1:p1"]["status"], state["w1:p1"]["due"]), ("done", 1130.0))

    def test_cancel_only_removes_the_target(self):
        self.store.add("w1:p1", "blocked", 60, now=0)
        self.store.add("w1:p2", "done", 60, now=0)
        self.assertEqual(self.store.cancel("w1:p1")["pane_id"], "w1:p1")
        self.assertIsNone(self.store.cancel("w1:p1"))
        self.assertEqual(list(self.store.read()), ["w1:p2"])
        self.assertEqual(len(self.store.cancel_all()), 1)
        self.assertEqual(self.store.read(), {})

    def test_missing_file_is_empty_and_corrupt_file_is_reported(self):
        self.assertEqual(self.store.read(), {})
        os.makedirs(self.dir)
        with open(self.store.path, "w") as handle:
            handle.write("{not json")
        with self.assertRaises(timer.StateError):
            self.store.read()

    def test_malformed_entries_are_ignored_not_fatal(self):
        os.makedirs(self.dir)
        good = {"pane_id": "w1:p1", "status": "done", "due": 5.0}
        bad_status = {"pane_id": "w1:p2", "status": "green", "due": 5.0}
        with open(self.store.path, "w") as handle:
            json.dump({"version": 1, "timers": {"w1:p1": good, "w1:p2": bad_status, "w1:p3": 7}}, handle)
        self.assertEqual(list(self.store.read()), ["w1:p1"])

    def test_remove_if_same_keeps_a_timer_replaced_meanwhile(self):
        old, _ = self.store.add("w1:p1", "blocked", 10, now=0)
        self.store.add("w1:p1", "done", 20, now=0)
        self.assertFalse(self.store.remove_if_same(old))
        self.assertEqual(self.store.read()["w1:p1"]["status"], "done")

    def test_concurrent_adds_lose_nothing(self):
        errors = []

        def worker(index):
            try:
                for n in range(15):
                    self.store.add("w%d:p%d" % (index, n), "blocked", 60, now=0)
            except Exception as exc:  # pragma: no cover - would fail the assertion below
                errors.append(exc)

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(6)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        self.assertEqual(errors, [])
        self.assertEqual(len(self.store.read()), 6 * 15)

    def test_atomic_write_survives_a_concurrent_reader(self):
        # Alternate between a tiny and a large document; a torn write would surface as
        # a StateError (truncated JSON) or as a count that is neither 1 nor 400.
        big = {"p%d" % i: {"pane_id": "p%d" % i, "status": "done", "due": float(i)} for i in range(400)}
        small = {"p0": big["p0"]}
        self.store.add("p0", "done", 0, now=0)
        stop = threading.Event()
        seen = []
        failures = []

        def reader():
            while not stop.is_set():
                try:
                    seen.append(len(self.store.read()))
                except timer.StateError as exc:
                    failures.append(str(exc))

        thread = threading.Thread(target=reader)
        thread.start()
        try:
            for i in range(150):
                with self.store.locked():
                    self.store._write(big if i % 2 == 0 else small)
        finally:
            stop.set()
            thread.join()
        self.assertEqual(failures, [])
        self.assertTrue(seen)
        self.assertLessEqual(set(seen), {1, 400})


class DueSelection(unittest.TestCase):
    def test_overdue_boundary_and_future(self):
        timers = {
            "a": {"pane_id": "a", "status": "done", "due": 100.0},
            "b": {"pane_id": "b", "status": "done", "due": 50.0},  # already overdue at daemon start
            "c": {"pane_id": "c", "status": "done", "due": 100.5},
            "d": {"pane_id": "d", "status": "done", "due": 99.0},
        }
        due = timer.due_timers(timers, 100.0)
        self.assertEqual([t["pane_id"] for t in due], ["b", "d", "a"])  # earliest first, boundary included
        self.assertEqual(timer.due_timers(timers, 49.9), [])


class DaemonLockCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.dir = os.path.join(self._tmp.name, "state")

    def test_two_contenders_exactly_one_holds(self):
        first, second = timer.DaemonLock(self.dir), timer.DaemonLock(self.dir)
        self.addCleanup(first.release)
        self.addCleanup(second.release)
        results = [first.acquire(), second.acquire()]
        self.assertEqual(sorted(results), [False, True])
        self.assertTrue(timer.DaemonLock.is_held(self.dir))
        self.assertEqual(timer.DaemonLock.holder_pid(self.dir), os.getpid())

    def test_loser_does_not_wipe_the_recorded_pid_and_lock_is_reusable(self):
        first, second = timer.DaemonLock(self.dir), timer.DaemonLock(self.dir)
        self.assertTrue(first.acquire())
        self.assertFalse(second.acquire())
        self.assertEqual(timer.DaemonLock.holder_pid(self.dir), os.getpid())
        first.release()
        self.assertFalse(timer.DaemonLock.is_held(self.dir))
        self.assertTrue(second.acquire())
        second.release()

    def test_a_separate_process_cannot_take_a_held_lock(self):
        holder = timer.DaemonLock(self.dir)
        self.assertTrue(holder.acquire())
        self.addCleanup(holder.release)
        code = (
            "import sys; sys.path.insert(0, %r); import timer; "
            "sys.exit(0 if timer.DaemonLock(%r).acquire() else 3)" % (PLUGIN_DIR, self.dir)
        )
        self.assertEqual(subprocess.run([sys.executable, "-c", code]).returncode, 3)
        holder.release()
        self.assertEqual(subprocess.run([sys.executable, "-c", code]).returncode, 0)

    def test_is_held_is_false_without_a_lock_file(self):
        self.assertFalse(timer.DaemonLock.is_held(self.dir))


class SocketGrace(unittest.TestCase):
    def test_pure_function(self):
        self.assertFalse(timer.socket_grace_exceeded(None, 1000.0, 60))
        self.assertFalse(timer.socket_grace_exceeded(100.0, 159.9, 60))
        self.assertTrue(timer.socket_grace_exceeded(100.0, 160.0, 60))
        self.assertTrue(timer.socket_grace_exceeded(100.0, 100.0, 0))

    def test_grace_env(self):
        self.assertEqual(timer.grace_from_env({}), 60.0)
        self.assertEqual(timer.grace_from_env({"PANE_TIMER_SOCKET_GRACE_SECONDS": "2"}), 2.0)
        with mock.patch.object(timer, "log"):
            self.assertEqual(timer.grace_from_env({"PANE_TIMER_SOCKET_GRACE_SECONDS": "soon"}), 60.0)
            self.assertEqual(timer.grace_from_env({"PANE_TIMER_SOCKET_GRACE_SECONDS": "-1"}), 60.0)


class FakeApi:
    def __init__(self):
        self.calls = []
        self.set_status_error = None
        self.ping_error = None
        self.on_set_status = None

    def set_status(self, pane_id, status):
        self.calls.append((pane_id, status))
        if self.on_set_status:
            self.on_set_status(pane_id, status)
        if self.set_status_error:
            raise self.set_status_error

    def ping(self):
        if self.ping_error:
            raise self.ping_error
        return {"type": "pong"}


class DaemonTicks(StoreCase):
    def setUp(self):
        super().setUp()
        self.api = FakeApi()
        self.lines = []
        self.daemon = timer.Daemon(self.store, self.api, grace_seconds=10, log_fn=self.lines.append, probe_interval=5)

    def test_due_timer_fires_once_and_is_removed(self):
        self.store.add("w1:p1", "blocked", 5, now=100.0)
        self.assertFalse(self.daemon.tick(104.0))
        self.assertEqual(self.api.calls, [])
        self.assertFalse(self.daemon.tick(105.0))
        self.assertEqual(self.api.calls, [("w1:p1", "blocked")])
        self.assertEqual(self.store.read(), {})
        self.daemon.tick(106.0)
        self.assertEqual(len(self.api.calls), 1)

    def test_timer_overdue_at_daemon_start_fires_on_the_first_tick(self):
        self.store.add("w1:p1", "done", 60, now=0.0)
        self.daemon.tick(3600.0)  # e.g. the laptop slept, or the daemon was down
        self.assertEqual(self.api.calls, [("w1:p1", "done")])

    def test_missing_pane_drops_timer_and_daemon_survives(self):
        self.store.add("w1:p1", "blocked", 0, now=0)
        self.store.add("w1:p2", "done", 0, now=0)
        self.api.set_status_error = bora_api.ApiError("pane_not_found", "pane w1:p1 not found")
        self.assertFalse(self.daemon.tick(1.0))
        self.assertEqual(self.store.read(), {})
        self.assertEqual(len(self.api.calls), 2)  # one bad pane does not stop the next timer
        self.assertTrue(any("no longer exists" in line for line in self.lines))

    def test_other_api_error_keeps_the_timer_and_logs_once(self):
        self.store.add("w1:p1", "blocked", 0, now=0)
        self.api.set_status_error = bora_api.ApiError("internal_error", "boom")
        for now in (1.0, 2.0, 3.0):
            self.assertFalse(self.daemon.tick(now))
        self.assertIn("w1:p1", self.store.read())
        self.assertEqual(len(self.api.calls), 3)  # retried every tick
        self.assertEqual(sum("failed" in line for line in self.lines), 1)
        self.api.set_status_error = None
        self.daemon.tick(4.0)
        self.assertEqual(self.store.read(), {})

    def test_unreachable_socket_keeps_timer_then_exits_after_grace(self):
        self.store.add("w1:p1", "blocked", 0, now=0)
        self.api.set_status_error = bora_api.SocketUnreachable("gone")
        self.api.ping_error = bora_api.SocketUnreachable("gone")
        self.assertFalse(self.daemon.tick(100.0))
        self.assertFalse(self.daemon.tick(109.0))
        self.assertIn("w1:p1", self.store.read())
        self.assertTrue(self.daemon.tick(110.0))

    def test_recovery_resets_the_grace_clock(self):
        self.store.add("w1:p1", "blocked", 0, now=0)
        self.api.set_status_error = bora_api.SocketUnreachable("gone")
        self.daemon.tick(100.0)
        self.api.set_status_error = None
        self.daemon.tick(105.0)  # server is back: fires
        self.assertEqual(self.store.read(), {})
        self.api.ping_error = bora_api.SocketUnreachable("gone again")
        self.assertFalse(self.daemon.tick(112.0))  # probe fails; clock restarts here, not at 100
        self.assertFalse(self.daemon.tick(121.0))
        self.assertTrue(self.daemon.tick(122.0))

    def test_idle_daemon_notices_a_dead_server_through_the_probe(self):
        self.api.ping_error = bora_api.SocketUnreachable("gone")
        self.assertFalse(self.daemon.tick(0.0))
        self.assertFalse(self.daemon.tick(9.0))
        self.assertTrue(self.daemon.tick(10.0))

    def test_reachable_idle_daemon_never_exits(self):
        for now in range(0, 500, 5):
            self.assertFalse(self.daemon.tick(float(now)))

    def test_timer_replaced_while_firing_survives(self):
        self.store.add("w1:p1", "blocked", 0, now=0)
        self.api.on_set_status = lambda pane, status: self.store.add(pane, "done", 300, now=1.0)
        self.daemon.tick(1.0)
        remaining = self.store.read()["w1:p1"]
        self.assertEqual((remaining["status"], remaining["due"]), ("done", 301.0))


class Targeting(unittest.TestCase):
    def test_priority_and_fallbacks(self):
        context = json.dumps({"focused_pane_id": "w2:p3"})
        self.assertEqual(timer.focused_pane({"PANE_TIMER_TARGET": "w9:p9", "HERDR_PLUGIN_CONTEXT_JSON": context}), "w9:p9")
        self.assertEqual(timer.focused_pane({"HERDR_PLUGIN_CONTEXT_JSON": context, "HERDR_PANE_ID": "w1:p1"}), "w2:p3")
        self.assertEqual(timer.focused_pane({"HERDR_PLUGIN_CONTEXT_JSON": "{}", "HERDR_PANE_ID": "w1:p1"}), "w1:p1")
        self.assertEqual(timer.focused_pane({"HERDR_PLUGIN_CONTEXT_JSON": "not json", "HERDR_PANE_ID": "w1:p1"}), "w1:p1")
        self.assertIsNone(timer.focused_pane({}))


class ApiClient(unittest.TestCase):
    def test_socket_path_is_never_guessed(self):
        for env in ({}, {"HERDR_SOCKET_PATH": ""}):
            with self.assertRaisesRegex(bora_api.SocketPathNotSet, "HERDR_SOCKET_PATH"):
                bora_api.socket_path(env)
        with self.assertRaises(bora_api.SocketPathNotSet):
            bora_api.call("ping", {}, env={})

    def _serve_once(self, path, reply):
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(path)
        server.listen(1)
        received = []

        def run():
            conn, _ = server.accept()
            buf = b""
            while not buf.endswith(b"\n"):
                buf += conn.recv(4096)
            received.append(json.loads(buf))
            conn.sendall((json.dumps(dict(reply, id=received[0]["id"])) + "\n").encode())
            conn.close()
            server.close()

        thread = threading.Thread(target=run)
        thread.start()
        self.addCleanup(thread.join)
        return received

    def test_success_error_and_unreachable(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "s.sock")
            received = self._serve_once(path, {"result": {"type": "ok"}})
            result = bora_api.pane_set_status("w1:p1", "blocked", env={"HERDR_SOCKET_PATH": path})
            self.assertEqual(result, {"type": "ok"})
            self.assertEqual(received[0]["method"], "pane.set_status")
            self.assertEqual(received[0]["params"], {"pane_id": "w1:p1", "status": "blocked"})

            path2 = os.path.join(tmp, "e.sock")
            self._serve_once(path2, {"error": {"code": "pane_not_found", "message": "nope"}})
            with self.assertRaises(bora_api.ApiError) as ctx:
                bora_api.pane_set_status("w9:p9", "done", env={"HERDR_SOCKET_PATH": path2})
            self.assertEqual(ctx.exception.code, "pane_not_found")

            with self.assertRaises(bora_api.SocketUnreachable):
                bora_api.ping(env={"HERDR_SOCKET_PATH": os.path.join(tmp, "absent.sock")})

    def test_clearing_a_pin_omits_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "s.sock")
            received = self._serve_once(path, {"result": {"type": "ok"}})
            bora_api.pane_set_status("w1:p1", None, env={"HERDR_SOCKET_PATH": path})
            self.assertEqual(received[0]["params"], {"pane_id": "w1:p1"})


RUNNING = lambda _env: "running"  # keeps prompt tests from ever spawning a real daemon


class ArmAndPrompt(StoreCase):
    def env(self):
        return {"HERDR_PLUGIN_STATE_DIR": self.dir, "HERDR_SOCKET_PATH": "/unused", "PANE_TIMER_TARGET": "w1:p1"}

    def test_arm_refuses_a_missing_pane_and_writes_nothing(self):
        with mock.patch.object(bora_api, "pane_get", side_effect=bora_api.ApiError("pane_not_found", "x")):
            with self.assertRaisesRegex(timer.CommandError, "does not exist"):
                timer.arm_timer("w9:p9", 60, "blocked", env=self.env())
        self.assertEqual(self.store.read(), {})

    def test_cli_add_rejects_garbage_before_touching_bora_or_state(self):
        for delay, status in [("0", "blocked"), ("abc", "done"), ("5m", "green")]:
            with mock.patch.object(bora_api, "pane_get") as pane_get, contextlib.redirect_stderr(io.StringIO()):
                code = timer.main(["add", "w1:p1", delay, status], env=self.env())
            self.assertEqual(code, 1)
            pane_get.assert_not_called()
        self.assertEqual(self.store.read(), {})

    def test_prompt_reasks_until_valid_then_arms(self):
        answers = iter(["abc", "0", "10m", "x", "d"])
        prompts = []

        def fake_input(prompt):
            prompts.append(prompt)
            return next(answers)

        out = io.StringIO()
        with mock.patch.object(bora_api, "pane_get", return_value={}):
            code = timer.run_prompt(self.env(), input_fn=fake_input, out=out, sleep=lambda _s: None, ensure=RUNNING)
        self.assertEqual(code, 0)
        self.assertEqual(sum(p.startswith("Delay") for p in prompts), 3)
        self.assertEqual(sum(p.startswith("Status") for p in prompts), 2)
        state = self.store.read()["w1:p1"]
        self.assertEqual(state["status"], "done")
        self.assertAlmostEqual(state["due"] - state["created"], 600, delta=1)
        self.assertIn("invalid delay", out.getvalue())
        self.assertIn("armed w1:p1: done", out.getvalue())

    def test_prompt_empty_delay_or_eof_cancels_without_arming(self):
        for behaviour in ([""], EOFError()):
            with mock.patch.object(bora_api, "pane_get", return_value={}):
                fn = (lambda _p: behaviour[0]) if isinstance(behaviour, list) else mock.Mock(side_effect=behaviour)
                code = timer.run_prompt(self.env(), input_fn=fn, out=io.StringIO(), sleep=lambda _s: None, ensure=RUNNING)
            self.assertEqual(code, 0)
            self.assertEqual(self.store.read(), {})

    def test_prompt_default_status_is_blocked_and_replaces(self):
        self.store.add("w1:p1", "done", 900)
        answers = iter(["1h30m", ""])
        out = io.StringIO()
        with mock.patch.object(bora_api, "pane_get", return_value={}):
            timer.run_prompt(self.env(), input_fn=lambda _p: next(answers), out=out, sleep=lambda _s: None, ensure=RUNNING)
        self.assertEqual(self.store.read()["w1:p1"]["status"], "blocked")
        self.assertIn("current timer: done", out.getvalue())
        self.assertIn("replaced previous timer", out.getvalue())


class EnsureDaemon(StoreCase):
    def env(self):
        return {"HERDR_PLUGIN_STATE_DIR": self.dir, "HERDR_SOCKET_PATH": "/unused"}

    def test_held_lock_means_no_spawn(self):
        holder = timer.DaemonLock(self.dir)
        self.assertTrue(holder.acquire())
        self.addCleanup(holder.release)
        spawned = []
        self.assertEqual(timer.ensure_daemon(self.env(), spawner=lambda env, directory: spawned.append(1)), "running")
        self.assertEqual(spawned, [])

    def test_free_lock_spawns_once_and_waits_for_the_lock(self):
        holders = []
        calls = []

        def spawner(env, directory):
            calls.append(directory)
            lock = timer.DaemonLock(directory)  # stands in for the daemon that takes the lock
            self.assertTrue(lock.acquire())
            holders.append(lock)

        self.addCleanup(lambda: [h.release() for h in holders])
        self.assertEqual(timer.ensure_daemon(self.env(), spawner=spawner), "started")
        self.assertEqual(calls, [self.dir])
        self.assertEqual(timer.ensure_daemon(self.env(), spawner=spawner), "running")
        self.assertEqual(len(calls), 1)

    def test_spawn_that_never_takes_the_lock_fails_loudly_and_points_at_the_log(self):
        clock = iter([0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0])
        with self.assertRaisesRegex(timer.CommandError, r"did not take its lock within 2s; see .*daemon\.log"):
            timer.ensure_daemon(self.env(), spawner=lambda e, d: None, wait_seconds=2.0,
                                sleep=lambda _s: None, clock=lambda: next(clock))

    def test_child_that_already_exited_fails_at_once_instead_of_waiting(self):
        class Dead:
            def poll(self):
                return 2

        slept = []
        with self.assertRaisesRegex(timer.CommandError, "exited with status 2"):
            timer.ensure_daemon(self.env(), spawner=lambda e, d: Dead(), wait_seconds=60.0,
                                sleep=slept.append, clock=lambda: 0.0)
        self.assertEqual(slept, [])

    def test_spawn_os_error_is_reported(self):
        def boom(env, directory):
            raise OSError("no such interpreter")

        with self.assertRaisesRegex(timer.CommandError, "could not start the timer daemon: no such interpreter"):
            timer.ensure_daemon(self.env(), spawner=boom)

    def test_add_arms_then_starts_the_daemon_and_a_failed_start_is_nonzero(self):
        spawned = []
        with mock.patch.object(bora_api, "pane_get", return_value={}), \
                mock.patch.object(timer, "spawn_daemon", side_effect=lambda e, d: spawned.append(d)), \
                mock.patch.object(timer, "DAEMON_START_WAIT_SECONDS", 0.2), \
                contextlib.redirect_stderr(io.StringIO()) as err, contextlib.redirect_stdout(io.StringIO()):
            code = timer.main(["add", "w1:p1", "5m", "blocked"], env=self.env())
        self.assertEqual(code, 1)  # the spawner "started" nothing, so the lock never appears
        self.assertEqual(spawned, [self.dir])
        self.assertIn("timer armed on w1:p1, but", err.getvalue())
        self.assertIn("w1:p1", self.store.read())  # the timer is kept: a later daemon will fire it

    def test_real_daemon_spawn_takes_the_lock_and_exits_when_the_socket_is_missing(self):
        # Spawns an actual detached daemon process; HERDR_SOCKET_PATH is unset, so it must
        # refuse to start (exit 2) without ever taking the lock, and say why in daemon.log.
        env = {k: v for k, v in os.environ.items() if k != "HERDR_SOCKET_PATH"}
        env["HERDR_PLUGIN_STATE_DIR"] = self.dir
        with self.assertRaisesRegex(timer.CommandError, r"exited with status 2 without taking its lock; see .*daemon\.log"):
            timer.ensure_daemon(env, wait_seconds=5.0)
        with open(os.path.join(self.dir, "daemon.log")) as handle:
            self.assertIn("HERDR_SOCKET_PATH is not set", handle.read())


if __name__ == "__main__":
    unittest.main()

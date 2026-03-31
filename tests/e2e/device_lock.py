"""HTTP lock-server client for e2e test device management.

Mirrors the protocol of scripts/claim-device.sh but runs in-process,
so locks are acquired and released via pytest fixtures — only the
devices actually needed by collected tests are claimed.

The lock server (scripts/device-lock-server.py) uses TTL-based leases
renewed by heartbeats.  This module handles heartbeats automatically in
a background thread.

Usage in conftest.py::

    lock = DeviceLock.claim(timeout=240)
    # ... use lock.port ...
    lock.release()  # stops heartbeat, releases server-side lock
"""

import json
import os
import socket
import threading
import time
from dataclasses import dataclass, field
from urllib.error import URLError
from urllib.request import Request, urlopen


DEFAULT_LOCK_URL = "http://localhost:7357"
HEARTBEAT_INTERVAL = 30  # seconds (well within 60s TTL)


def _lock_url() -> str:
    return os.environ.get("DEVICE_LOCK_URL", DEFAULT_LOCK_URL)


def _holder_info() -> dict:
    """Build holder metadata (matches claim-device.sh format)."""
    container = ""
    if os.path.isfile("/proc/1/cpuset"):
        try:
            with open("/proc/1/cpuset") as f:
                container = f.read().strip()
        except OSError:
            pass
    elif os.path.isfile("/.dockerenv"):
        container = socket.gethostname()
    return {
        "pid": os.getpid(),
        "host": socket.gethostname(),
        "container": container or "none",
        "time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def _http_json(method: str, url: str, body: dict | None = None,
               timeout: float = 10) -> tuple[int, dict]:
    """Make an HTTP request and return (status_code, parsed_json)."""
    data = json.dumps(body).encode() if body else None
    req = Request(url, data=data, method=method)
    if data:
        req.add_header("Content-Type", "application/json")
    try:
        with urlopen(req, timeout=timeout) as resp:
            return resp.status, json.loads(resp.read())
    except URLError as e:
        if hasattr(e, "code"):
            # HTTPError — has a readable body
            try:
                return e.code, json.loads(e.read())
            except Exception:
                return e.code, {"error": str(e)}
        raise


@dataclass
class DeviceLock:
    """A lock held on the HTTP lock server with automatic heartbeats.

    Use ``claim()`` to acquire and ``release()`` to free.  Also works as
    a context manager.
    """

    port: str
    lock_id: str
    _heartbeat_stop: threading.Event = field(default_factory=threading.Event, repr=False)
    _heartbeat_thread: threading.Thread | None = field(default=None, repr=False)

    def release(self) -> None:
        """Release the lock on the server and stop heartbeating."""
        # Stop heartbeat thread
        if self._heartbeat_stop is not None:
            self._heartbeat_stop.set()
        if self._heartbeat_thread is not None:
            self._heartbeat_thread.join(timeout=5)
            self._heartbeat_thread = None

        # Release server-side lock
        if self.lock_id:
            try:
                _http_json("DELETE", f"{_lock_url()}/lock/{self.lock_id}")
            except Exception:
                pass  # Server will expire it via TTL
            self.lock_id = ""

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.release()

    def __del__(self):
        self.release()

    # -- Factory ---------------------------------------------------------------

    @classmethod
    def claim(
        cls,
        device: str | None = None,
        exclude: set[str] | None = None,
        timeout: float = 120,
        interval: float = 5,
    ) -> "DeviceLock":
        """Claim a device from the lock server.

        Parameters
        ----------
        device:
            Specific device path to lock, or None for any available.
        exclude:
            Device paths to skip (e.g. already-claimed DUT).
        timeout:
            Maximum wait in seconds.
        interval:
            Seconds between retry attempts.

        Returns
        -------
        DeviceLock
            With ``.port`` and ``.lock_id`` set, heartbeat running.

        Raises
        ------
        RuntimeError
            If no device could be claimed within *timeout*.
        FileNotFoundError
            If no USB serial devices exist at all.
        ConnectionError
            If the lock server is unreachable.
        """
        base_url = _lock_url()
        holder = _holder_info()
        exclude = exclude or set()

        # If a specific device is pinned, use that
        request_device = device or "any"

        deadline = time.monotonic() + timeout

        while True:
            try:
                body = {"device": request_device, "holder": holder}
                status, data = _http_json("POST", f"{base_url}/lock", body)
            except (URLError, OSError) as e:
                raise ConnectionError(
                    f"Cannot reach lock server at {base_url}: {e}"
                ) from e

            if status == 200:
                claimed_device = data["device"]
                # If this device is in the exclude set, release and retry
                if claimed_device in exclude:
                    lock_id = data["lock_id"]
                    try:
                        _http_json("DELETE", f"{base_url}/lock/{lock_id}")
                    except Exception:
                        pass
                    # Fall through to retry
                else:
                    return cls._from_server_response(data)

            elif status == 404:
                err = data.get("error", "")
                if "no devices" in err:
                    raise FileNotFoundError("No USB serial devices found")
                # Device not found — might be a race, retry
            elif status == 409:
                pass  # All devices busy, retry
            else:
                pass  # Unexpected status, retry

            if time.monotonic() >= deadline:
                raise RuntimeError(
                    f"Could not claim a device within {timeout}s "
                    f"(last: {data.get('error', 'unknown')})"
                )
            time.sleep(interval)

    @classmethod
    def _from_server_response(cls, data: dict) -> "DeviceLock":
        """Create a DeviceLock from a successful /lock response."""
        lock_id = data["lock_id"]
        port = data["device"]

        stop_event = threading.Event()
        lock = cls(
            port=port,
            lock_id=lock_id,
            _heartbeat_stop=stop_event,
        )

        # Start heartbeat thread
        thread = threading.Thread(
            target=cls._heartbeat_loop,
            args=(lock_id, stop_event),
            daemon=True,
            name=f"heartbeat-{lock_id}",
        )
        thread.start()
        lock._heartbeat_thread = thread
        return lock

    @staticmethod
    def _heartbeat_loop(lock_id: str, stop_event: threading.Event):
        """Send heartbeats every HEARTBEAT_INTERVAL until stopped.

        Each heartbeat includes the current PID so the server can verify
        the holder is still alive via the OS process table.

        Transient network errors are retried up to ``MAX_HEARTBEAT_FAILURES``
        consecutive times before giving up — a single blip won't kill the
        lock while the test is still running.
        """
        MAX_HEARTBEAT_FAILURES = 3
        base_url = _lock_url()
        consecutive_failures = 0
        while not stop_event.wait(HEARTBEAT_INTERVAL):
            try:
                status, _ = _http_json(
                    "POST",
                    f"{base_url}/lock/{lock_id}/heartbeat",
                    body={"pid": os.getpid()},
                    timeout=10,
                )
                if status == 200:
                    consecutive_failures = 0
                    continue
                if status == 404:
                    break  # Lock gone (expired or released), stop
                # Other status — treat as transient
                consecutive_failures += 1
            except Exception:
                consecutive_failures += 1
            if consecutive_failures >= MAX_HEARTBEAT_FAILURES:
                break  # Server persistently unreachable, stop

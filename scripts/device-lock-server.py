#!/usr/bin/env python3
"""HTTP-based device lock server for coordinating USB device access.

Replaces flock-based locking with a centralized HTTP service that works
across independent git clones, worktrees, and Docker containers.

Usage:
    python3 scripts/device-lock-server.py                    # default :7357
    python3 scripts/device-lock-server.py --port 8080        # custom port
    python3 scripts/device-lock-server.py --ttl 120          # 2-min lease
    python3 scripts/device-lock-server.py --no-tui           # plain log output

Endpoints:
    POST   /lock                  Claim a device
    DELETE /lock/<lock_id>        Release a device
    POST   /lock/<lock_id>/heartbeat  Renew lease
    GET    /devices               List devices and lock status
    GET    /health                Health check
"""

import argparse
import datetime
import glob
import json
import os
import random
import shutil
import sys
import threading
import time
import uuid
from http.server import HTTPServer, BaseHTTPRequestHandler


def _pid_alive(pid: int) -> bool:
    """Check if a process is alive via /proc or kill(0).

    Only checks local processes — PIDs from containers on the same host
    are in the same PID namespace when the lock server runs on the host.
    """
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True  # Process exists but we can't signal it


class LockManager:
    """Thread-safe in-memory device lock manager with TTL-based expiry."""

    def __init__(self, ttl: int = 60):
        self.ttl = ttl
        self._locks: dict[str, dict] = {}  # lock_id -> lock info
        self._device_locks: dict[str, str] = {}  # device -> lock_id
        self._mu = threading.Lock()

    def _scan_devices(self) -> list[str]:
        devices = sorted(
            glob.glob("/dev/ttyUSB*") + glob.glob("/dev/ttyACM*")
        )
        return devices

    def acquire(self, device: str | None, holder: dict) -> dict | None:
        """Acquire a lock. device=None means pick any free device.

        Returns lock info dict on success, None if no device available.
        """
        with self._mu:
            self._reap_expired()
            devices = self._scan_devices()
            if not devices:
                return {"error": "no devices found", "code": 404}

            if device and device != "any":
                # Pinned device request
                targets = [device] if device in devices else []
                if not targets:
                    return {"error": f"device {device} not found", "code": 404}
            else:
                # Shuffle for fairness
                targets = list(devices)
                random.shuffle(targets)

            for dev in targets:
                if dev not in self._device_locks:
                    lock_id = uuid.uuid4().hex[:12]
                    now = time.time()
                    info = {
                        "lock_id": lock_id,
                        "device": dev,
                        "holder": holder,
                        "acquired_at": now,
                        "last_heartbeat": now,
                        "expires_at": now + self.ttl,
                    }
                    self._locks[lock_id] = info
                    self._device_locks[dev] = lock_id
                    return info

            return {"error": "all devices locked", "code": 409}

    def release(self, lock_id: str) -> bool:
        with self._mu:
            info = self._locks.pop(lock_id, None)
            if info:
                self._device_locks.pop(info["device"], None)
                return True
            return False

    def heartbeat(self, lock_id: str, pid: int | None = None) -> dict | None:
        """Renew a lock's lease.  *pid* is required — it must be the PID of
        the process sending the heartbeat so that liveness can be verified
        via the OS process table.

        Returns a status dict on success, or None if the lock doesn't exist.
        """
        with self._mu:
            info = self._locks.get(lock_id)
            if not info:
                return None
            if pid is None:
                return {"error": "pid is required", "code": 400}
            now = time.time()
            info["last_heartbeat"] = now
            info["expires_at"] = now + self.ttl
            info["holder"]["pid"] = pid
            return {"status": "renewed"}

    def list_devices(self) -> dict:
        with self._mu:
            self._reap_expired()
            devices = self._scan_devices()
            result = []
            for dev in devices:
                lock_id = self._device_locks.get(dev)
                if lock_id and lock_id in self._locks:
                    info = self._locks[lock_id]
                    result.append({
                        "device": dev,
                        "status": "locked",
                        "lock_id": lock_id,
                        "holder": info["holder"],
                        "acquired_at": info["acquired_at"],
                        "expires_at": info["expires_at"],
                    })
                else:
                    result.append({"device": dev, "status": "free"})

            locked = sum(1 for d in result if d["status"] == "locked")
            return {
                "devices": result,
                "total": len(result),
                "locked": locked,
                "free": len(result) - locked,
            }

    def _reap_expired(self):
        """Remove expired locks and locks held by dead processes.

        Must be called with self._mu held.
        """
        now = time.time()
        to_remove: list[tuple[str, str]] = []  # (lock_id, reason)
        for lid, info in self._locks.items():
            if info["expires_at"] <= now:
                to_remove.append((lid, "expired"))
                continue
            # Check if the heartbeating process is still alive
            pid = info.get("holder", {}).get("pid")
            if pid is not None and not _pid_alive(int(pid)):
                to_remove.append((lid, f"dead pid {pid}"))
        for lid, reason in to_remove:
            info = self._locks.pop(lid)
            self._device_locks.pop(info["device"], None)
            print(f"[reaper] {reason}: released lock {lid} on {info['device']}")


def make_handler(manager: LockManager, quiet: bool = False):

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            if not quiet:
                print(f"[http] {self.address_string()} {format % args}")

        def _send_json(self, code: int, data: dict):
            body = json.dumps(data).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _read_body(self) -> bytes:
            length = int(self.headers.get("Content-Length", 0))
            return self.rfile.read(length) if length else b""

        def _parse_path(self) -> tuple[str, list[str]]:
            """Returns (base_path, segments)."""
            path = self.path.rstrip("/")
            segments = [s for s in path.split("/") if s]
            return path, segments

        def do_GET(self):
            path, segments = self._parse_path()
            if path == "/devices":
                self._send_json(200, manager.list_devices())
            elif path == "/health":
                self._send_json(200, {"status": "ok"})
            else:
                self._send_json(404, {"error": "not found"})

        def do_POST(self):
            path, segments = self._parse_path()
            if path == "/lock":
                body = self._read_body()
                try:
                    data = json.loads(body) if body else {}
                except json.JSONDecodeError:
                    self._send_json(400, {"error": "invalid JSON"})
                    return

                device = data.get("device")
                holder = data.get("holder", {})
                result = manager.acquire(device, holder)
                if "error" in result:
                    self._send_json(result["code"], {"error": result["error"]})
                else:
                    self._send_json(200, result)

            elif len(segments) == 3 and segments[0] == "lock" and segments[2] == "heartbeat":
                lock_id = segments[1]
                body = self._read_body()
                try:
                    data = json.loads(body) if body else {}
                except json.JSONDecodeError:
                    data = {}
                pid = data.get("pid")
                if pid is not None:
                    try:
                        pid = int(pid)
                    except (ValueError, TypeError):
                        self._send_json(400, {"error": "pid must be an integer"})
                        return
                result = manager.heartbeat(lock_id, pid=pid)
                if result is None:
                    self._send_json(404, {"error": "lock not found"})
                elif "error" in result:
                    self._send_json(result["code"], {"error": result["error"]})
                else:
                    self._send_json(200, result)
            else:
                self._send_json(404, {"error": "not found"})

        def do_DELETE(self):
            path, segments = self._parse_path()
            if len(segments) == 2 and segments[0] == "lock":
                lock_id = segments[1]
                if manager.release(lock_id):
                    self._send_json(200, {"status": "released"})
                else:
                    self._send_json(404, {"error": "lock not found"})
            else:
                self._send_json(404, {"error": "not found"})

    return Handler


def reaper_loop(manager: LockManager, interval: int = 10):
    """Background thread that periodically reaps expired locks."""
    while True:
        time.sleep(interval)
        # list_devices triggers _reap_expired under the lock
        manager.list_devices()


# ── TUI ────────────────────────────────────────────────────────────────────

# ANSI helpers
CSI = "\033["
HIDE_CURSOR = f"{CSI}?25l"
SHOW_CURSOR = f"{CSI}?25h"
CLEAR_SCREEN = f"{CSI}2J{CSI}H"
BOLD = f"{CSI}1m"
DIM = f"{CSI}2m"
RESET = f"{CSI}0m"
GREEN = f"{CSI}32m"
RED = f"{CSI}31m"
YELLOW = f"{CSI}33m"
CYAN = f"{CSI}36m"


def _bar(fraction: float, width: int) -> str:
    """Render a progress-style bar: [████░░░░]"""
    filled = int(fraction * width)
    empty = width - filled
    color = GREEN if fraction < 0.6 else YELLOW if fraction < 0.9 else RED
    return f"{DIM}[{RESET}{color}{'█' * filled}{'░' * empty}{RESET}{DIM}]{RESET}"


def _fmt_duration(seconds: float) -> str:
    """Format seconds as human-readable duration."""
    if seconds < 0:
        return "expired"
    m, s = divmod(int(seconds), 60)
    h, m = divmod(m, 60)
    if h:
        return f"{h}h{m:02d}m"
    if m:
        return f"{m}m{s:02d}s"
    return f"{s}s"


def _render_frame(manager: LockManager, port: int, ttl: int) -> str:
    """Render one frame of the TUI status display."""
    now = time.time()
    data = manager.list_devices()
    devices = data["devices"]
    total = data["total"]
    locked = data["locked"]
    free = data["free"]

    cols = shutil.get_terminal_size((80, 24)).columns
    lines: list[str] = []

    # Header
    ts = datetime.datetime.now().strftime("%H:%M:%S")
    title = f" Device Lock Server :{port} "
    lines.append(f"{BOLD}{CYAN}{title}{RESET}{DIM} TTL={ttl}s  {ts}{RESET}")
    lines.append(f"{DIM}{'─' * min(cols, 72)}{RESET}")

    # Summary bar
    frac = locked / total if total else 0
    bar = _bar(frac, 20)
    lines.append(f"  {bar}  {BOLD}{free}{RESET} free  {BOLD}{locked}{RESET} locked  {DIM}({total} total){RESET}")
    lines.append("")

    if not devices:
        lines.append(f"  {DIM}No USB serial devices found{RESET}")
    else:
        # Column header
        lines.append(f"  {DIM}{'DEVICE':<20} {'STATUS':<8} {'HOLDER':<32} {'TTL':>6}  {'HELD':>8}{RESET}")
        lines.append(f"  {DIM}{'─' * 20} {'─' * 8} {'─' * 32} {'─' * 6}  {'─' * 8}{RESET}")

        for d in devices:
            dev = d["device"]
            short = dev.split("/")[-1]
            if d["status"] == "free":
                lines.append(f"  {short:<20} {GREEN}{'FREE':<8}{RESET}")
            else:
                h = d.get("holder", {})
                holder_parts = []
                if h.get("host"):
                    holder_parts.append(str(h["host"]))
                if h.get("container") and h["container"] != "none" and h["container"] != h.get("host"):
                    holder_parts.append(f"ctr:{h['container']}")
                if h.get("pid"):
                    holder_parts.append(f"pid:{h['pid']}")
                holder_str = " ".join(holder_parts)[:32]

                expires = d.get("expires_at", now)
                ttl_left = expires - now
                ttl_str = _fmt_duration(ttl_left)
                ttl_color = GREEN if ttl_left > 20 else YELLOW if ttl_left > 5 else RED

                acquired = d.get("acquired_at", now)
                held_str = _fmt_duration(now - acquired)

                lines.append(
                    f"  {short:<20} {RED}{'LOCKED':<8}{RESET} "
                    f"{holder_str:<32} {ttl_color}{ttl_str:>6}{RESET}  {DIM}{held_str:>8}{RESET}"
                )

    lines.append("")
    lines.append(f"  {DIM}Ctrl-C to quit{RESET}")
    return "\n".join(lines)


def tui_loop(manager: LockManager, port: int, ttl: int):
    """Run the TUI status display, refreshing every second."""
    sys.stdout.write(HIDE_CURSOR)
    sys.stdout.flush()
    try:
        while True:
            frame = _render_frame(manager, port, ttl)
            sys.stdout.write(CLEAR_SCREEN + frame)
            sys.stdout.flush()
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        sys.stdout.write(SHOW_CURSOR + "\n")
        sys.stdout.flush()


def main():
    parser = argparse.ArgumentParser(description="Device lock server")
    parser.add_argument("--port", type=int, default=7357, help="Listen port (default: 7357)")
    parser.add_argument("--ttl", type=int, default=60, help="Lock TTL in seconds (default: 60)")
    parser.add_argument("--no-tui", action="store_true", help="Disable live TUI, log to stdout instead")
    args = parser.parse_args()

    manager = LockManager(ttl=args.ttl)

    # Start background reaper
    reaper = threading.Thread(target=reaper_loop, args=(manager,), daemon=True)
    reaper.start()

    use_tui = not args.no_tui and sys.stdout.isatty()

    server = HTTPServer(("0.0.0.0", args.port), make_handler(manager, quiet=use_tui))
    http_thread = threading.Thread(target=server.serve_forever, daemon=True)
    http_thread.start()

    if use_tui:
        print(f"Device lock server listening on :{args.port} (TTL={args.ttl}s)")
        # Brief pause so the server is ready before TUI takes over stdout
        time.sleep(0.2)
        try:
            tui_loop(manager, args.port, args.ttl)
        finally:
            server.shutdown()
    else:
        print(f"Device lock server listening on :{args.port} (TTL={args.ttl}s)")
        try:
            http_thread.join()
        except KeyboardInterrupt:
            print("\nShutting down")
            server.shutdown()


if __name__ == "__main__":
    main()

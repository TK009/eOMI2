#!/usr/bin/env python3
"""HTTP-based device lock server for coordinating USB device access.

Replaces flock-based locking with a centralized HTTP service that works
across independent git clones, worktrees, and Docker containers.

Usage:
    python3 scripts/device-lock-server.py                    # default :7357
    python3 scripts/device-lock-server.py --port 8080        # custom port
    python3 scripts/device-lock-server.py --ttl 120          # 2-min lease

Endpoints:
    POST   /lock                  Claim a device
    DELETE /lock/<lock_id>        Release a device
    POST   /lock/<lock_id>/heartbeat  Renew lease
    GET    /devices               List devices and lock status
    GET    /health                Health check
"""

import argparse
import glob
import json
import random
import threading
import time
import uuid
from http.server import HTTPServer, BaseHTTPRequestHandler


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

    def heartbeat(self, lock_id: str) -> bool:
        with self._mu:
            info = self._locks.get(lock_id)
            if info:
                now = time.time()
                info["last_heartbeat"] = now
                info["expires_at"] = now + self.ttl
                return True
            return False

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
        """Remove expired locks. Must be called with self._mu held."""
        now = time.time()
        expired = [
            lid for lid, info in self._locks.items()
            if info["expires_at"] <= now
        ]
        for lid in expired:
            info = self._locks.pop(lid)
            self._device_locks.pop(info["device"], None)
            print(f"[reaper] expired lock {lid} on {info['device']}")


def make_handler(manager: LockManager):

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
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
                if manager.heartbeat(lock_id):
                    self._send_json(200, {"status": "renewed"})
                else:
                    self._send_json(404, {"error": "lock not found"})
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


def main():
    parser = argparse.ArgumentParser(description="Device lock server")
    parser.add_argument("--port", type=int, default=7357, help="Listen port (default: 7357)")
    parser.add_argument("--ttl", type=int, default=60, help="Lock TTL in seconds (default: 60)")
    args = parser.parse_args()

    manager = LockManager(ttl=args.ttl)

    # Start background reaper
    reaper = threading.Thread(target=reaper_loop, args=(manager,), daemon=True)
    reaper.start()

    server = HTTPServer(("0.0.0.0", args.port), make_handler(manager))
    print(f"Device lock server listening on :{args.port} (TTL={args.ttl}s)")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down")
        server.shutdown()


if __name__ == "__main__":
    main()

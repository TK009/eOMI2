"""Serial bridge client for the WiFi bridge firmware.

Communicates with an ESP32 running the bridge firmware over UART,
forwarding HTTP requests to the DUT's soft-AP portal.
"""

import json
import time

import serial


class SerialBridge:
    """Serial bridge to an ESP32 running wifi-bridge firmware."""

    def __init__(self, port, baud=115200, ready_timeout=15):
        self._ser = serial.Serial(port, baud, timeout=1)
        self._wait_ready(ready_timeout)

    def _wait_ready(self, timeout):
        """Wait for the bridge firmware to print the ready message."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            resp = self._read_response(timeout=1)
            if resp and resp.get("type") == "ready":
                return
        raise TimeoutError(
            f"Bridge did not become ready within {timeout}s"
        )

    def _read_response(self, timeout=10):
        """Read lines until a !-prefixed JSON response is found."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            self._ser.timeout = min(remaining, 1.0)
            raw = self._ser.readline()
            if not raw:
                continue
            line = raw.decode("utf-8", errors="replace").strip()
            if line.startswith("!"):
                return json.loads(line[1:])
        return None

    def send_command(self, cmd_dict, timeout=15):
        """Send a JSON command and return the response dict."""
        line = json.dumps(cmd_dict, separators=(",", ":")) + "\n"
        self._ser.write(line.encode("utf-8"))
        self._ser.flush()
        resp = self._read_response(timeout=timeout)
        if resp is None:
            raise TimeoutError("No response from bridge")
        if not resp.get("ok"):
            raise RuntimeError(f"Bridge error: {resp.get('error', 'unknown')}")
        return resp

    def ping(self):
        return self.send_command({"cmd": "ping"})

    def scan(self):
        resp = self.send_command({"cmd": "scan"}, timeout=20)
        return resp.get("networks", [])

    def connect(self, ssid, password=""):
        return self.send_command(
            {"cmd": "connect", "ssid": ssid, "pass": password}, timeout=20
        )

    def disconnect(self):
        return self.send_command({"cmd": "disconnect"})

    def status(self):
        return self.send_command({"cmd": "status"})

    def http_get(self, url, timeout=15, authorization=""):
        cmd = {"cmd": "http", "method": "GET", "url": url}
        if authorization:
            cmd["authorization"] = authorization
        return self.send_command(cmd, timeout=timeout)

    def http_post(self, url, body="", content_type="", timeout=15,
                  authorization=""):
        cmd = {"cmd": "http", "method": "POST", "url": url}
        if body:
            cmd["body"] = body
        if content_type:
            cmd["content_type"] = content_type
        if authorization:
            cmd["authorization"] = authorization
        return self.send_command(cmd, timeout=timeout)

    def close(self):
        if self._ser and self._ser.is_open:
            self._ser.close()

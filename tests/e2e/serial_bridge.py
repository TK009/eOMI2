"""Serial bridge client for the WiFi bridge firmware.

Communicates with an ESP32 running the bridge firmware over UART,
forwarding HTTP requests to the DUT's soft-AP portal.
"""

import json
import re
import time

import serial

# Strip ANSI escape sequences (ESP-IDF log colours) from serial output.
_ANSI_RE = re.compile(r'\x1b\[[0-9;]*m')


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
                # Strip ANSI escape codes that may leak from ESP-IDF log
                # output interleaved with the JSON response on UART0.
                clean = _ANSI_RE.sub('', line[1:])
                # Extract the JSON object — log output may be appended.
                brace = clean.find('{')
                if brace < 0:
                    continue
                clean = clean[brace:]
                depth = 0
                end = 0
                for i, c in enumerate(clean):
                    if c == '{':
                        depth += 1
                    elif c == '}':
                        depth -= 1
                        if depth == 0:
                            end = i + 1
                            break
                if end == 0:
                    continue
                try:
                    return json.loads(clean[:end])
                except json.JSONDecodeError:
                    continue
        return None

    def drain(self):
        """Discard any stale data sitting in the host-side serial buffer.

        Call this after a sequence of commands that may have timed out on the
        host side while the bridge was still processing.  Without draining,
        the next send_command would read a stale response from a previous
        command instead of the new one.
        """
        self._ser.reset_input_buffer()

    def send_command(self, cmd_dict, timeout=15, _retries=2):
        """Send a JSON command and return the response dict.

        Drains any stale data from the serial buffer before sending to
        prevent reading a response from a previously timed-out command.
        Retries once on 'unknown command' errors, which can occur when
        UART data from a previous timed-out exchange corrupts the line.
        """
        # Discard stale responses from commands that timed out on our
        # side but were still processed by the bridge.
        self._ser.reset_input_buffer()
        line = json.dumps(cmd_dict, separators=(",", ":")) + "\n"
        data = line.encode("utf-8")
        # Send in chunks to avoid overflowing the ESP32's UART RX FIFO
        # (128 bytes hardware) when the firmware is busy.
        chunk = 64
        for i in range(0, len(data), chunk):
            self._ser.write(data[i:i + chunk])
            self._ser.flush()
            if i + chunk < len(data):
                time.sleep(0.005)  # 5 ms between chunks
        resp = self._read_response(timeout=timeout)
        if resp is None:
            raise TimeoutError("No response from bridge")
        if not resp.get("ok"):
            err = resp.get("error", "unknown")
            if err == "unknown command" and _retries > 0:
                # UART desync — drain and retry.
                time.sleep(0.5)
                self._ser.reset_input_buffer()
                return self.send_command(cmd_dict, timeout=timeout,
                                         _retries=_retries - 1)
            raise RuntimeError(f"Bridge error: {err}")
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

    def connect_by_prefix(self, prefix, password="", timeout=20):
        """Scan for an AP whose SSID starts with *prefix* and connect."""
        networks = self.scan()
        for net in networks:
            if net.get("ssid", "").startswith(prefix):
                return self.connect(net["ssid"], password)
        raise RuntimeError(f"No AP found with prefix '{prefix}'")

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

    def reset(self):
        """Hardware-reset the bridge ESP32 via DTR/RTS toggle."""
        # Flush any pending writes so they don't arrive after reboot.
        self._ser.reset_output_buffer()
        self._ser.dtr = False
        self._ser.rts = True
        time.sleep(0.1)
        self._ser.rts = False
        time.sleep(0.1)
        self._ser.reset_input_buffer()
        self._wait_ready(15)
        # Drain any remaining boot output that arrives after the ready message.
        time.sleep(0.5)
        self._ser.reset_input_buffer()

    def close(self):
        if self._ser and self._ser.is_open:
            self._ser.close()

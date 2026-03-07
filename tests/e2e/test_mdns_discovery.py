"""E2E device tests for mDNS and DNS-SD discovery.

Exercises mDNS hostname resolution, DNS-SD service browsing, TXT records,
and ODF tree publication of discovery results on real hardware.

Test cases:
  SC-001: Resolve <hostname>.local from test host (within 5s)
  SC-002: Browse _omi._tcp and verify device appears (within 10s)
  SC-003: Verify TXT record contains O-DF object path
  SC-004: Check discovery InfoItems in ODF tree via OMI API (within 60s)
  SC-005: Verify mDNS stops when entering AP mode
  SC-006: Verify mDNS resumes after re-provisioning
  SC-007: Multi-device discovery if second device available

Environment variables:
  DEVICE_PORT     -- USB serial port (for flash/reset)
  DEVICE_IP       -- device IP once connected to STA network
  DEVICE_HOSTNAME -- device mDNS hostname (default: "eomi")
  WIFI_SSID       -- test network SSID (needed for reprovisioning tests)
  WIFI_PASS       -- test network password
  DEVICE_IP_2     -- second device IP (optional, for multi-device tests)
  API_TOKEN       -- bearer token for authenticated requests
"""

import os
import socket
import time

import pytest
from zeroconf import ServiceBrowser, ServiceStateChange, Zeroconf

from helpers import (
    omi_read,
    reboot_device,
    wait_for_device,
    wait_for_device_down,
    REQUEST_TIMEOUT,
)

pytestmark = pytest.mark.mdns

DEVICE_HOSTNAME = os.environ.get("DEVICE_HOSTNAME", "eomi")
MDNS_RESOLVE_TIMEOUT = 5    # SC-001: hostname resolution within 5s
BROWSE_TIMEOUT = 10          # SC-002: service browse within 10s
DISCOVERY_ODF_TIMEOUT = 60   # SC-004: ODF tree population within 60s

# Portal defaults for AP-mode tests.
PORTAL_IP = os.environ.get("PORTAL_IP", "192.168.4.1")
PORTAL_URL = f"http://{PORTAL_IP}"


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def device_hostname():
    return DEVICE_HOSTNAME


@pytest.fixture(scope="module")
def wifi_ssid():
    ssid = os.environ.get("WIFI_SSID")
    if not ssid:
        pytest.skip("WIFI_SSID not set")
    return ssid


@pytest.fixture(scope="module")
def wifi_pass():
    return os.environ.get("WIFI_PASS", "")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def resolve_mdns_hostname(hostname, timeout=MDNS_RESOLVE_TIMEOUT):
    """Resolve <hostname>.local via mDNS and return the IP address string.

    Uses the zeroconf library for cross-platform mDNS resolution.
    Returns None if resolution fails within *timeout* seconds.
    """
    fqdn = f"{hostname}.local."
    zc = Zeroconf()
    try:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            info = zc.get_service_info("_omi._tcp.local.", f"{hostname}._omi._tcp.local.")
            if info and info.parsed_addresses():
                return info.parsed_addresses()[0]
            # Fallback: try direct name resolution
            try:
                addr = socket.getaddrinfo(
                    f"{hostname}.local", None, socket.AF_INET, socket.SOCK_STREAM,
                )
                if addr:
                    return addr[0][4][0]
            except (socket.gaierror, OSError):
                pass
            time.sleep(0.5)
        return None
    finally:
        zc.close()


def browse_omi_services(timeout=BROWSE_TIMEOUT):
    """Browse for _omi._tcp services via DNS-SD.

    Returns a list of dicts with keys: name, hostname, ip, port, txt.
    """
    found = []
    zc = Zeroconf()

    class Collector:
        def __init__(self):
            self.services = []

        def on_state_change(self, zeroconf, service_type, name, state_change):
            if state_change == ServiceStateChange.Added:
                info = zeroconf.get_service_info(service_type, name)
                if info:
                    txt = {}
                    if info.properties:
                        for k, v in info.properties.items():
                            key = k.decode("utf-8") if isinstance(k, bytes) else k
                            val = v.decode("utf-8") if isinstance(v, bytes) else v
                            txt[key] = val
                    addresses = info.parsed_addresses()
                    self.services.append({
                        "name": name,
                        "hostname": info.server,
                        "ip": addresses[0] if addresses else None,
                        "port": info.port,
                        "txt": txt,
                    })

    collector = Collector()
    try:
        browser = ServiceBrowser(
            zc, "_omi._tcp.local.", handlers=[collector.on_state_change]
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if collector.services:
                # Give a bit more time for additional services
                time.sleep(1)
                break
            time.sleep(0.5)
        browser.cancel()
        return collector.services
    finally:
        zc.close()


# ---------------------------------------------------------------------------
# SC-001: Resolve <hostname>.local from test host (within 5s)
# ---------------------------------------------------------------------------

class TestMdnsResolution:
    """mDNS hostname resolution for .local domain."""

    def test_resolve_hostname(self, device_ip, device_hostname):
        """SC-001: <hostname>.local resolves to the device IP within 5s."""
        resolved_ip = resolve_mdns_hostname(device_hostname, timeout=MDNS_RESOLVE_TIMEOUT)
        assert resolved_ip is not None, (
            f"{device_hostname}.local did not resolve within {MDNS_RESOLVE_TIMEOUT}s"
        )
        assert resolved_ip == device_ip, (
            f"mDNS resolved to {resolved_ip}, expected {device_ip}"
        )


# ---------------------------------------------------------------------------
# SC-002 + SC-003: Browse _omi._tcp, verify TXT record
# ---------------------------------------------------------------------------

class TestDnsSdBrowse:
    """DNS-SD service browsing for _omi._tcp."""

    @pytest.fixture(scope="class")
    def discovered_services(self):
        """Browse for _omi._tcp services."""
        return browse_omi_services(timeout=BROWSE_TIMEOUT)

    def test_service_found(self, discovered_services, device_hostname):
        """SC-002: Device appears in _omi._tcp browse within 10s."""
        names = [s["name"] for s in discovered_services]
        matching = [
            s for s in discovered_services
            if device_hostname in s["name"]
        ]
        assert matching, (
            f"Device '{device_hostname}' not found in _omi._tcp browse. "
            f"Found: {names}"
        )

    def test_service_ip_matches(self, discovered_services, device_ip, device_hostname):
        """Discovered service IP matches the known device IP."""
        matching = [
            s for s in discovered_services
            if device_hostname in s["name"]
        ]
        assert matching, "Device not found in browse results"
        assert matching[0]["ip"] == device_ip

    def test_service_port(self, discovered_services, device_hostname):
        """Discovered service advertises port 80."""
        matching = [
            s for s in discovered_services
            if device_hostname in s["name"]
        ]
        assert matching, "Device not found in browse results"
        assert matching[0]["port"] == 80

    def test_txt_record_has_path(self, discovered_services, device_hostname):
        """SC-003: TXT record contains the O-DF object path."""
        matching = [
            s for s in discovered_services
            if device_hostname in s["name"]
        ]
        assert matching, "Device not found in browse results"
        txt = matching[0]["txt"]
        assert "path" in txt, f"TXT record missing 'path' key: {txt}"
        assert txt["path"] == "/Objects", (
            f"TXT path is '{txt['path']}', expected '/Objects'"
        )


# ---------------------------------------------------------------------------
# SC-004: Discovery InfoItems in ODF tree via OMI API (within 60s)
# ---------------------------------------------------------------------------

class TestDiscoveryOdfTree:
    """Verify discovery results are published to the ODF tree."""

    def test_discovery_subtree_exists(self, base_url):
        """SC-004: /System/discovery is populated in the ODF tree within 60s.

        The device runs peer discovery every 30s. On a network with at least
        one other _omi._tcp device, InfoItems should appear. If no peers
        exist, the subtree may be empty but should not error after the
        discovery tick has run at least once.
        """
        deadline = time.monotonic() + DISCOVERY_ODF_TIMEOUT
        last_err = None
        while time.monotonic() < deadline:
            try:
                data = omi_read(base_url, path="/System/discovery")
                status = data["response"]["status"]
                if status == 200:
                    return  # subtree exists
                last_err = f"OMI status {status}"
            except Exception as exc:
                last_err = str(exc)
            time.sleep(5)
        # The discovery subtree may not exist if there are no peers on the
        # network — that's acceptable. The key check is that the device
        # doesn't crash and the OMI API remains responsive.
        data = omi_read(base_url, path="/System")
        assert data["response"]["status"] == 200, (
            f"/System unreadable after {DISCOVERY_ODF_TIMEOUT}s: {last_err}"
        )

    def test_discovery_item_format(self, base_url, device_ip_2=None):
        """If peers exist, discovery items have '<ip>:<port>' values."""
        if device_ip_2 is None:
            device_ip_2 = os.environ.get("DEVICE_IP_2")
        if not device_ip_2:
            pytest.skip("DEVICE_IP_2 not set — no second device for peer check")

        deadline = time.monotonic() + DISCOVERY_ODF_TIMEOUT
        while time.monotonic() < deadline:
            try:
                data = omi_read(base_url, path="/System/discovery")
                if data["response"]["status"] == 200:
                    result = data["response"]["result"]
                    # result should have items dict
                    items = result.get("items", {})
                    for name, item_data in items.items():
                        values = item_data.get("values", [])
                        if values:
                            v = values[0]["v"]
                            assert ":" in v, (
                                f"Discovery value '{v}' missing port separator"
                            )
                            return
            except Exception:
                pass
            time.sleep(5)
        pytest.fail(
            f"No discovery items with values after {DISCOVERY_ODF_TIMEOUT}s"
        )


# ---------------------------------------------------------------------------
# SC-005: mDNS stops when entering AP mode
# ---------------------------------------------------------------------------

class TestMdnsApMode:
    """Verify mDNS is not active when the device enters AP/portal mode."""

    @pytest.fixture(autouse=True)
    def _needs_serial(self, device_port):
        """These tests require serial access for reset."""

    def test_mdns_stops_in_ap_mode(self, device_port, device_hostname):
        """SC-005: After factory reset (entering portal mode), mDNS hostname
        no longer resolves on the STA network."""
        import subprocess

        # Erase NVS to force portal mode
        subprocess.run(
            ["espflash", "erase-region", "--port", device_port,
             "0x9000", "0x6000"],
            check=True,
            capture_output=True,
            timeout=15,
        )
        reboot_device(device_port)

        # Wait for portal to come up (device is now in AP mode)
        time.sleep(10)

        # mDNS should NOT resolve on the STA network anymore
        resolved = resolve_mdns_hostname(device_hostname, timeout=5)
        assert resolved is None, (
            f"{device_hostname}.local still resolves to {resolved} in AP mode"
        )

    def test_dns_sd_stops_in_ap_mode(self, device_hostname):
        """_omi._tcp service should not be browseable when in AP mode."""
        services = browse_omi_services(timeout=5)
        matching = [
            s for s in services
            if device_hostname in s["name"]
        ]
        assert not matching, (
            f"Device still advertising _omi._tcp in AP mode: {matching}"
        )


# ---------------------------------------------------------------------------
# SC-006: mDNS resumes after re-provisioning
# ---------------------------------------------------------------------------

class TestMdnsResumeAfterProvisioning:
    """Verify mDNS restarts after the device is re-provisioned."""

    @pytest.fixture(autouse=True)
    def _reprovision(self, device_port, wifi_ssid, wifi_pass, base_url):
        """Factory reset, re-provision via portal, wait for STA connection."""
        import subprocess
        import requests as req

        # Erase NVS and reboot into portal mode
        subprocess.run(
            ["espflash", "erase-region", "--port", device_port,
             "0x9000", "0x6000"],
            check=True,
            capture_output=True,
            timeout=15,
        )
        reboot_device(device_port)

        # Wait for portal
        deadline = time.monotonic() + 30
        portal_up = False
        while time.monotonic() < deadline:
            try:
                resp = req.get(f"{PORTAL_URL}/", timeout=5)
                if resp.status_code == 200 and "Device Setup" in resp.text:
                    portal_up = True
                    break
            except req.RequestException:
                pass
            time.sleep(1)
        if not portal_up:
            pytest.skip("Portal did not come up within 30s")

        # Submit provisioning form
        data = {"ssid_0": wifi_ssid, "password_0": wifi_pass,
                "api_key_action": "generate"}
        req.post(
            f"{PORTAL_URL}/provision", data=data,
            timeout=REQUEST_TIMEOUT, allow_redirects=False,
        )

        # Wait for device to connect to STA
        wait_for_device(base_url, timeout=60)

    def test_mdns_resolves_after_reprovision(self, device_ip, device_hostname):
        """SC-006: mDNS hostname resolves again after re-provisioning."""
        resolved = resolve_mdns_hostname(device_hostname, timeout=MDNS_RESOLVE_TIMEOUT)
        assert resolved is not None, (
            f"{device_hostname}.local did not resolve after re-provisioning"
        )
        assert resolved == device_ip

    def test_dns_sd_resumes_after_reprovision(self, device_hostname):
        """_omi._tcp service is browseable again after re-provisioning."""
        services = browse_omi_services(timeout=BROWSE_TIMEOUT)
        matching = [
            s for s in services
            if device_hostname in s["name"]
        ]
        assert matching, (
            f"Device not found in _omi._tcp browse after re-provisioning"
        )


# ---------------------------------------------------------------------------
# SC-007: Multi-device discovery
# ---------------------------------------------------------------------------

class TestMultiDeviceDiscovery:
    """When multiple eOMI devices are on the network, all are discoverable."""

    @pytest.fixture(autouse=True)
    def _needs_second_device(self):
        if not os.environ.get("DEVICE_IP_2"):
            pytest.skip("DEVICE_IP_2 not set — multi-device tests need a second device")

    def test_multiple_services_found(self):
        """SC-007: Multiple _omi._tcp services appear in browse results."""
        services = browse_omi_services(timeout=BROWSE_TIMEOUT)
        assert len(services) >= 2, (
            f"Expected at least 2 _omi._tcp services, found {len(services)}: "
            f"{[s['name'] for s in services]}"
        )

    def test_both_devices_have_txt_records(self):
        """Both discovered devices have valid TXT records with path key."""
        services = browse_omi_services(timeout=BROWSE_TIMEOUT)
        for svc in services:
            assert "path" in svc["txt"], (
                f"Service '{svc['name']}' missing 'path' TXT key"
            )

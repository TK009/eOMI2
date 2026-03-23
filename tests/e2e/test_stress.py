"""Section 13 — Stress & Stability Tests.

Rapid writes, concurrent connections, large payloads, and sustained
operation over time.
"""

import concurrent.futures
import json
import logging
import time
import warnings

import pytest
import requests

from helpers import omi_delete, omi_read, omi_write, omi_write_tree, omi_status, wait_for_device

log = logging.getLogger(__name__)

pytestmark = pytest.mark.stress


@pytest.fixture(autouse=True, scope="module")
def cleanup_stress_paths(base_url, token):
    """Remove /Stress after all stress tests."""
    yield
    try:
        omi_delete(base_url, "/Stress", token=token)
    except Exception as exc:
        warnings.warn(f"Stress cleanup failed (stale /Stress tree may remain): {exc}")


def test_rapid_writes(base_url, token):
    """Send 100 sequential overwrites in quick succession; device stays responsive.

    No delay between writes — intentionally provides zero backpressure to
    stress the device's request pipeline.  Some writes may time out under
    load; we tolerate that as long as the device recovers.
    """
    timeouts = 0
    for i in range(100):
        try:
            data = omi_write(base_url, "/Stress/Rapid", i, token=token)
            assert data["response"]["status"] in (200, 201), f"write {i} failed: {data}"
        except (requests.exceptions.ReadTimeout, requests.exceptions.ConnectionError):
            timeouts += 1
            if timeouts > 20:
                pytest.fail(f"Too many timeouts ({timeouts}) during rapid writes at iteration {i}")
            time.sleep(1)  # brief backoff after error

    # Device must still be responsive after the burst — give it time to recover
    wait_for_device(base_url, timeout=60)
    health = omi_read(base_url, "/", token=token, timeout=30)
    assert omi_status(health) == 200


def test_concurrent_reads(base_url, token):
    """5 simultaneous reads must all succeed."""
    def do_read():
        return omi_read(base_url, "/", token=token, timeout=30)

    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
        futures = [pool.submit(do_read) for _ in range(5)]
        results = [f.result() for f in futures]

    for i, data in enumerate(results):
        assert omi_status(data) == 200, f"concurrent read {i} failed: {data}"


def test_concurrent_mixed(base_url, token):
    """5 simultaneous mixed read/write operations must all succeed."""
    def do_read(idx):
        return ("read", idx, omi_read(base_url, "/", token=token, timeout=30))

    def do_write(idx):
        return ("write", idx, omi_write(base_url, f"/Stress/Concurrent/{idx}", idx, token=token))

    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
        futures = [
            pool.submit(do_write, 0),
            pool.submit(do_read, 1),
            pool.submit(do_write, 2),
            pool.submit(do_read, 3),
            pool.submit(do_write, 4),
        ]
        results = [f.result() for f in futures]

    for op, idx, data in results:
        status = omi_status(data)
        if op == "read":
            assert status == 200, f"concurrent read {idx} failed: {data}"
        else:
            assert status in (200, 201), f"concurrent write {idx} failed: {data}"


def test_large_payload(base_url, token):
    """Write a large nested tree; device accepts or gracefully rejects it."""
    # Build a deeply nested tree (~10 levels) with long IDs to produce
    # a large JSON payload without creating many NVS entries at once.
    inner = {"id": "Leaf" + "_pad" * 20}
    for depth in range(10):
        name = f"Level{depth:02d}" + "_pad" * 15
        inner = {"id": name, "objects": {name: inner}}
    objects = {"Stress": {"id": "Stress", "objects": {inner["id"]: inner}}}

    payload_size = len(json.dumps(objects).encode())
    log.info("large-payload tree size: %d bytes", payload_size)

    try:
        data = omi_write_tree(base_url, "/", objects, token=token, timeout=30)
        status = omi_status(data)
        assert status in (200, 201, 400, 413), f"unexpected status {status}: {data}"
    except (requests.exceptions.ReadTimeout, requests.exceptions.HTTPError,
            requests.exceptions.ConnectionError):
        # Device could not process the payload in time or rejected it —
        # acceptable for a stress test; verify it recovers below.
        pass

    # Device must still be responsive after the large payload attempt
    wait_for_device(base_url, timeout=30)
    health = omi_read(base_url, "/", token=token)
    assert omi_status(health) == 200


def test_long_running(base_url, token):
    """Write + read loop for 2 minutes; device must stay responsive."""
    for i in range(120):
        w = omi_write(base_url, "/Stress/Long", i, token=token)
        assert w["response"]["status"] in (200, 201), f"write {i} failed: {w}"

        r = omi_read(base_url, "/Stress/Long", token=token, newest=1)
        assert omi_status(r) == 200, f"read {i} failed: {r}"
        # Allow for NVS write-back delay: the read may still return the
        # previous value under load.
        v = r["response"]["result"]["values"][0]["v"]
        assert v in (i, i - 1), f"read {i}: unexpected value {v}"

        time.sleep(1)

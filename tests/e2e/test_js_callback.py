"""Section 008 — Javascript:// callback subscription E2E tests.

Tests for javascript:// callback subscriptions on real hardware:
  SC-001: Interval subscription with javascript:// callback executes script
  SC-002: Event subscription with javascript:// callback fires on write
  SC-004: No network traffic generated for javascript:// callbacks
"""

import subprocess
import time

import pytest

from helpers import (
    TREE_WRITE_TIMEOUT,
    omi_delete,
    omi_read,
    omi_write,
    omi_write_tree,
    omi_status,
    omi_result,
    wait_for_values,
)

# Subscription tick fires at 100ms cadence but next_sub_trigger is only
# refreshed on the 5s main tick. Worst case: subscription created just after
# a 5s tick → 5s wait for refresh + 5s interval = 10s before first fire.
# Add margin for slow ESP32-S2 processing under load (many tree items).
TICK_WAIT = 18


@pytest.fixture(autouse=True, scope="module")
def cleanup_test_paths(base_url, token):
    """Remove test objects after all javascript:// callback tests."""
    yield
    for path in ("/JsCb", "/Callbacks"):
        try:
            omi_delete(base_url, path, token=token)
        except Exception:
            pass


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def store_callback_script(base_url, script_name, script_src, token):
    """Store a script in /Callbacks/MetaData/{script_name} via tree write.

    The corresponding javascript:// URL is:
        javascript:///Callbacks/MetaData/{script_name}
    """
    objects = {
        "Callbacks": {
            "id": "Callbacks",
            "objects": {
                "MetaData": {
                    "id": "MetaData",
                    "items": {
                        script_name: {
                            "values": [{"v": script_src}],
                        },
                    },
                },
            },
        },
    }
    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201), (
        f"failed to store callback script '{script_name}': {data}"
    )


def create_writable_item(base_url, path, initial_value, token):
    """Create a writable item via tree write with initial value."""
    # Split path into object/item parts: /JsCb/Dst -> object=JsCb, item=Dst
    parts = [p for p in path.split("/") if p]
    if len(parts) < 2:
        raise ValueError(f"path must have at least object/item: {path}")

    # Build nested objects structure
    item_name = parts[-1]
    item_def = {
        "values": [{"v": initial_value}],
        "meta": {"writable": True},
    }

    # Build from innermost to outermost
    current = {"items": {item_name: item_def}}
    current["id"] = parts[-2]

    for i in range(len(parts) - 3, -1, -1):
        parent = {"id": parts[i], "objects": {parts[i + 1]: current}}
        current = parent

    root_key = parts[0]
    objects = {root_key: current}

    data = omi_write_tree(base_url, "/", objects, token=token, timeout=TREE_WRITE_TIMEOUT)
    assert data["response"]["status"] in (200, 201), (
        f"failed to create writable item {path}: {data}"
    )


def js_callback_url(script_name):
    """Return the javascript:// URL for a named callback script."""
    return f"javascript:///Callbacks/MetaData/{script_name}"


def omi_subscribe_callback(base_url, path, callback_url, interval=-1, ttl=60,
                            token=None):
    """Create a subscription with a callback URL (javascript:// or http://).

    Returns parsed JSON response.
    """
    payload = {
        "omi": "1.0",
        "ttl": ttl,
        "read": {
            "path": path,
            "interval": interval,
            "callback": callback_url,
        },
    }
    import requests
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    resp = requests.post(
        f"{base_url}/omi",
        json=payload,
        headers=headers,
        timeout=TREE_WRITE_TIMEOUT,
    )
    resp.raise_for_status()
    return resp.json()


# ===========================================================================
# SC-001: Interval subscription with javascript:// callback
# ===========================================================================


def test_interval_js_callback_executes_script(base_url, token):
    """Interval subscription with javascript:// callback executes the script
    periodically, writing computed values to a destination path."""
    # 1. Store callback script: reads source value and writes it +1 to dest
    store_callback_script(
        base_url, "on_interval",
        "var v = odf.readItem('/JsCb/Src/value'); odf.writeItem(v + 1, '/JsCb/IntervalDst');",
        token=token,
    )

    # 2. Create source and destination items
    create_writable_item(base_url, "/JsCb/Src", 10, token)
    create_writable_item(base_url, "/JsCb/IntervalDst", 0, token)

    # 3. Create interval subscription with javascript:// callback
    data = omi_subscribe_callback(
        base_url, "/JsCb/Src",
        js_callback_url("on_interval"),
        interval=5,  # fire every 5 seconds
        ttl=60,
        token=token,
    )
    assert data["response"]["status"] == 200, (
        f"subscribe failed: {data}"
    )

    # 4. Wait for at least one tick to fire and the callback to execute
    time.sleep(TICK_WAIT)

    # 5. Verify the callback script wrote to the destination
    read = omi_read(base_url, "/JsCb/IntervalDst", token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1, "expected callback to have written a value"
    assert values[0]["v"] == 11, (
        f"expected script to write 10+1=11, got {values[0]['v']}"
    )


def test_interval_js_callback_fires_multiple_ticks(base_url, token):
    """Interval javascript:// callback fires multiple times, each tick
    incrementing a counter."""
    # Script increments the destination value each time it fires
    store_callback_script(
        base_url, "counter",
        "var c = odf.readItem('/JsCb/Counter/value'); odf.writeItem(c + 1, '/JsCb/Counter');",
        token=token,
    )

    create_writable_item(base_url, "/JsCb/Counter", 0, token)

    data = omi_subscribe_callback(
        base_url, "/JsCb/Counter",
        js_callback_url("counter"),
        interval=3,  # fire every 3 seconds
        ttl=30,
        token=token,
    )
    assert data["response"]["status"] == 200

    # Wait for multiple ticks (3s interval × ~3 ticks ≈ 12s with margin)
    time.sleep(TICK_WAIT)

    read = omi_read(base_url, "/JsCb/Counter", token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    # Counter should have been incremented at least twice
    assert values[0]["v"] >= 2, (
        f"expected counter >= 2 after multiple ticks, got {values[0]['v']}"
    )


# ===========================================================================
# SC-002: Event subscription with javascript:// callback fires on write
# ===========================================================================


def test_event_js_callback_fires_on_write(base_url, token):
    """Event subscription with javascript:// callback fires when the
    subscribed path is written, executing the callback script."""
    # Script copies the written value to a destination
    store_callback_script(
        base_url, "on_event",
        "odf.writeItem(event.values[0].value, '/JsCb/EventDst');",
        token=token,
    )

    create_writable_item(base_url, "/JsCb/EventSrc", 0, token)
    create_writable_item(base_url, "/JsCb/EventDst", 0, token)

    # Create event subscription (interval=-1) with javascript:// callback
    data = omi_subscribe_callback(
        base_url, "/JsCb/EventSrc",
        js_callback_url("on_event"),
        interval=-1,
        ttl=60,
        token=token,
    )
    assert data["response"]["status"] == 200

    # Write to the subscribed path — triggers the callback
    write_data = omi_write(base_url, "/JsCb/EventSrc", 42, token=token)
    assert write_data["response"]["status"] in (200, 201)

    # The callback should have written 42 to the destination.
    # Allow a brief settle time for the dispatch loop.
    time.sleep(1)

    read = omi_read(base_url, "/JsCb/EventDst", token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1, "expected callback to have written to EventDst"
    assert values[0]["v"] == 42, (
        f"expected callback to copy value 42, got {values[0]['v']}"
    )


def test_event_js_callback_correct_values(base_url, token):
    """Event javascript:// callback receives the correct written value and
    can transform it."""
    # Script doubles the value
    store_callback_script(
        base_url, "doubler",
        "odf.writeItem(event.values[0].value * 2, '/JsCb/Doubled');",
        token=token,
    )

    create_writable_item(base_url, "/JsCb/DoublerSrc", 0, token)
    create_writable_item(base_url, "/JsCb/Doubled", 0, token)

    data = omi_subscribe_callback(
        base_url, "/JsCb/DoublerSrc",
        js_callback_url("doubler"),
        interval=-1,
        ttl=60,
        token=token,
    )
    assert data["response"]["status"] == 200

    # Write 21 → script doubles to 42
    omi_write(base_url, "/JsCb/DoublerSrc", 21, token=token)
    time.sleep(1)

    read = omi_read(base_url, "/JsCb/Doubled", token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == 42, (
        f"expected doubled value 42, got {values[0]['v']}"
    )


def test_event_js_callback_cascade(base_url, token):
    """Event javascript:// callback can trigger further event subscriptions
    via cascading writeItem calls."""
    # Chain: write to /JsCb/CascA → callback copies to /JsCb/CascB
    #        /JsCb/CascB has its own event sub → callback copies to /JsCb/CascC
    store_callback_script(
        base_url, "casc_a",
        "odf.writeItem(event.values[0].value, '/JsCb/CascB');",
        token=token,
    )
    store_callback_script(
        base_url, "casc_b",
        "odf.writeItem(event.values[0].value, '/JsCb/CascC');",
        token=token,
    )

    create_writable_item(base_url, "/JsCb/CascA", 0, token)
    create_writable_item(base_url, "/JsCb/CascB", 0, token)
    create_writable_item(base_url, "/JsCb/CascC", 0, token)

    # Subscribe CascA → casc_a script
    data = omi_subscribe_callback(
        base_url, "/JsCb/CascA",
        js_callback_url("casc_a"),
        interval=-1, ttl=60, token=token,
    )
    assert data["response"]["status"] == 200

    # Subscribe CascB → casc_b script
    data = omi_subscribe_callback(
        base_url, "/JsCb/CascB",
        js_callback_url("casc_b"),
        interval=-1, ttl=60, token=token,
    )
    assert data["response"]["status"] == 200

    # Trigger the chain
    omi_write(base_url, "/JsCb/CascA", 99, token=token)
    time.sleep(2)

    # CascC should have received the cascaded value
    read = omi_read(base_url, "/JsCb/CascC", token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == 99, (
        f"expected cascaded value 99 at CascC, got {values[0]['v']}"
    )


# ===========================================================================
# SC-004: No network traffic for javascript:// callbacks
# ===========================================================================


def test_js_callback_no_network_traffic(base_url, token, device_ip):
    """javascript:// callbacks stay local — no HTTP traffic is generated.

    Uses tcpdump to capture packets on the device IP during callback
    execution and verifies no outbound HTTP requests are made.
    """
    # Store a script that writes locally (no HTTP needed)
    store_callback_script(
        base_url, "local_only",
        "odf.writeItem(event.values[0].value, '/JsCb/LocalDst');",
        token=token,
    )

    create_writable_item(base_url, "/JsCb/LocalSrc", 0, token)
    create_writable_item(base_url, "/JsCb/LocalDst", 0, token)

    # Create event subscription with javascript:// callback
    data = omi_subscribe_callback(
        base_url, "/JsCb/LocalSrc",
        js_callback_url("local_only"),
        interval=-1, ttl=60, token=token,
    )
    assert data["response"]["status"] == 200

    # Start packet capture for traffic FROM the device (port 80 or any HTTP)
    # We capture for a short window around the write that triggers the callback
    tcpdump_proc = None
    pcap_file = "/tmp/js_callback_capture.pcap"
    try:
        tcpdump_proc = subprocess.Popen(
            [
                "tcpdump", "-i", "any",
                "-w", pcap_file,
                "-c", "100",  # max packets to capture
                f"src host {device_ip} and tcp and not port 22",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        # Give tcpdump time to start capturing
        time.sleep(1)

        # Trigger the javascript:// callback by writing to the source
        omi_write(base_url, "/JsCb/LocalSrc", 77, token=token)

        # Wait for callback to execute
        time.sleep(3)

    finally:
        if tcpdump_proc:
            tcpdump_proc.terminate()
            tcpdump_proc.wait(timeout=5)

    # Verify the callback executed (script wrote to LocalDst)
    read = omi_read(base_url, "/JsCb/LocalDst", token=token, newest=1)
    assert omi_status(read) == 200
    values = omi_result(read)["values"]
    assert len(values) >= 1
    assert values[0]["v"] == 77, "callback script should have written the value"

    # Analyze the capture: count packets FROM device that are NOT responses
    # to our own HTTP requests (i.e., SYN packets initiated by the device)
    try:
        result = subprocess.run(
            [
                "tcpdump", "-r", pcap_file,
                "-nn",
                f"src host {device_ip} and tcp[tcpflags] & tcp-syn != 0 "
                f"and tcp[tcpflags] & tcp-ack == 0",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        outbound_syns = result.stdout.strip()
        if outbound_syns:
            lines = outbound_syns.split("\n")
            pytest.fail(
                f"Device initiated {len(lines)} outbound TCP connection(s) "
                f"during javascript:// callback — expected none.\n"
                f"Connections:\n{outbound_syns}"
            )
    except FileNotFoundError:
        pytest.skip("tcpdump not available for packet capture verification")
    except subprocess.TimeoutExpired:
        pytest.skip("tcpdump analysis timed out")


# ===========================================================================
# Additional: Script error resilience on device
# ===========================================================================


def test_js_callback_script_error_device_stays_responsive(base_url, token):
    """A broken callback script does not crash the device or block writes."""
    # Store a script with a syntax error
    store_callback_script(
        base_url, "broken",
        "this is not valid javascript!!!",
        token=token,
    )

    create_writable_item(base_url, "/JsCb/ErrSrc", 0, token)

    # Subscribe with broken callback
    data = omi_subscribe_callback(
        base_url, "/JsCb/ErrSrc",
        js_callback_url("broken"),
        interval=-1, ttl=60, token=token,
    )
    assert data["response"]["status"] == 200

    # Write — triggers the broken script, but write should succeed
    write_data = omi_write(base_url, "/JsCb/ErrSrc", 123, token=token)
    assert write_data["response"]["status"] in (200, 201)

    # Device still responsive
    read = omi_read(base_url, "/", token=token)
    assert omi_status(read) == 200

    # Subsequent writes still work
    write_data2 = omi_write(base_url, "/JsCb/ErrSrc", 456, token=token)
    assert write_data2["response"]["status"] in (200, 201)

    # Can read the value back
    read2 = omi_read(base_url, "/JsCb/ErrSrc", token=token, newest=1)
    assert omi_status(read2) == 200
    assert omi_result(read2)["values"][0]["v"] == 456

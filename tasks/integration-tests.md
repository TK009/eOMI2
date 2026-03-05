# Integration Test Plan

Integration tests verify that modules work correctly **together** across boundaries.
Unit tests (307+ existing) cover individual modules; these tests cover the gaps between them.

All tests run on host via `cargo test-host --test <name>` (no hardware needed).

---

## Guiding Principles

- **Test at system boundaries**: exercise the public `Engine` API with full JSON round-trips, not internal helpers.
- **Minimal mocking**: wire real `Engine` + `ObjectTree` + `SubscriptionRegistry` together. Only the HTTP transport and hardware are absent.
- **Independent tests**: each test constructs its own `Engine` — no shared mutable state, no ordering dependencies.
- **Assert on observable output**: check the JSON response (status, result, rid), not internal struct fields.
- **Keep them fast**: pure in-process Rust, no I/O, no threads.

---

## Test File Structure

```
tests/
  memory_budget.rs          # (existing) ELF size verification
  engine_integration.rs     # Engine round-trips: parse JSON → process → check JSON response
  subscription_lifecycle.rs # Multi-step subscription scenarios across engine calls
  http_routing.rs           # HTTP helper functions wired with Engine (uri→path→read→response)
```

Each file is a separate integration test binary (Rust convention).

---

## 1. `tests/engine_integration.rs` — End-to-End Request Processing

Tests the full path: **JSON string → `OmiMessage::parse` → `Engine::process` → response JSON**.

### Helper

```rust
fn engine_with_sensor_tree() -> Engine {
    let mut eng = Engine::new();
    eng.tree.write_tree("/", device::build_sensor_tree()).unwrap();
    eng
}
```

### 1.1 Read Operations

| Test | Description |
|------|-------------|
| `read_root_returns_objects` | `GET /` equivalent — read path `/`, verify response contains `Dht11` object |
| `read_object_returns_items` | Read `/Dht11`, verify `Temperature` and `RelativeHumidity` items present |
| `read_infoitem_empty` | Read `/Dht11/Temperature` with no values written — 200 with empty values |
| `read_infoitem_with_values` | Write a value, then read — verify the value appears in response |
| `read_newest_oldest_filters` | Write 5 values, read with `newest=2` and `oldest=2`, verify counts |
| `read_time_range` | Write values at different timestamps, read with `begin`/`end`, verify filtering |
| `read_with_depth` | Read `/` with `depth=1`, verify items are not expanded |
| `read_nonexistent_path` | Read `/NoSuchThing` — verify 404 status in response |

### 1.2 Write Operations

| Test | Description |
|------|-------------|
| `write_new_path_creates_item` | Write to `/MyObj/MyItem`, verify 201, then read it back |
| `write_existing_writable` | Create item via write (auto-writable), write again — verify 200, value updated |
| `write_read_only_rejected` | Write to `/Dht11/Temperature` (sensor-owned, not writable) — verify 403 |
| `write_batch_mixed_results` | Batch write: one to writable path, one to read-only — verify partial success response |
| `write_tree_merges_objects` | Write an object subtree, verify it merges into existing tree |
| `write_then_read_roundtrip` | Write a string, number, bool, null — read each back, verify values preserved |

### 1.3 Delete Operations

| Test | Description |
|------|-------------|
| `delete_existing_object` | Delete `/Dht11`, verify 200, then read — verify 404 |
| `delete_root_forbidden` | Delete `/` — verify 403 |
| `delete_nonexistent` | Delete `/Ghost` — verify 404 |

### 1.4 Cancel Operations

| Test | Description |
|------|-------------|
| `cancel_nonexistent_rid` | Cancel `["rid-999"]` — verify 404 |

### 1.5 Error Handling

| Test | Description |
|------|-------------|
| `malformed_json_rejected` | Pass invalid JSON to `OmiMessage::parse` — verify `ParseError` |
| `missing_operation_rejected` | `{"omi":"1.0","ttl":0}` with no op — verify parse error |
| `wrong_version_rejected` | `{"omi":"2.0",...}` — verify parse error |

---

## 2. `tests/subscription_lifecycle.rs` — Multi-Step Subscription Scenarios

Tests that span **multiple `Engine::process` calls** over simulated time.

### Helper

```rust
fn now() -> f64 { 1_000_000.0 }  // fixed base timestamp for reproducibility
```

### 2.1 Poll Subscription

| Test | Description |
|------|-------------|
| `poll_sub_create_write_poll` | Create poll sub on `/Dht11/Temperature` → write a value → tick → poll by rid → verify value delivered |
| `poll_sub_drain_clears_buffer` | Poll returns values, second poll returns empty |
| `poll_sub_multiple_values` | Write 3 values between polls — verify all 3 returned |
| `poll_sub_ttl_expiry` | Create sub with `ttl=60`, advance time by 61s, poll — verify 404 (expired) |

### 2.2 Event Subscription

| Test | Description |
|------|-------------|
| `event_sub_triggers_on_write` | Create event sub (interval=-1) with callback target → write value → check delivery returned by `notify_event` |
| `event_sub_no_trigger_on_unrelated_write` | Sub on `/A`, write to `/B` — verify no delivery |

### 2.3 Interval Subscription

| Test | Description |
|------|-------------|
| `interval_sub_fires_on_tick` | Create interval sub (interval=10s) → advance time by 10s → `tick()` → verify delivery |
| `interval_sub_skips_before_due` | Create interval sub → tick at t+5s — verify no delivery |

### 2.4 WebSocket Subscription

| Test | Description |
|------|-------------|
| `ws_sub_cancelled_on_disconnect` | Create sub with `ws_session=Some(42)` → cancel by session → verify sub gone |

### 2.5 Cancel

| Test | Description |
|------|-------------|
| `cancel_stops_delivery` | Create event sub → cancel → write value → verify no delivery |
| `cancel_batch` | Create 3 subs → cancel 2 → verify 1 remains |

---

## 3. `tests/http_routing.rs` — HTTP Helpers + Engine Wiring

Tests the chain: **URI + query params → ODF path → read operation → Engine → response**.
Exercises functions from `http.rs` wired with a real `Engine` — the closest we can get to HTTP integration without ESP transport.

### 3.1 REST Discovery

| Test | Description |
|------|-------------|
| `get_omi_root` | `uri_to_odf_path("/omi/")` → build read op → process → verify root objects listed |
| `get_omi_object` | `/omi/Dht11/` → verify Temperature and Humidity items |
| `get_omi_infoitem` | `/omi/Dht11/Temperature` → verify value response |
| `get_omi_with_query_params` | `/omi/Dht11/Temperature?newest=3&depth=1` → verify params applied |

### 3.2 Landing Page

| Test | Description |
|------|-------------|
| `landing_page_lists_pages` | Add pages to `PageStore`, call `render_landing_page` — verify HTML contains links |

### 3.3 Authentication Boundary

| Test | Description |
|------|-------------|
| `read_not_mutating` | Verify `is_mutating_operation` returns false for reads |
| `write_is_mutating` | Verify `is_mutating_operation` returns true for write, delete, cancel, subscription |

---

## 4. `tests/scripting_integration.rs` — Script Engine + ODF

Tests the chain: **write with on-write script → script executes → cascading write**.
Only compiles with `features = ["scripting"]`.

| Test | Description |
|------|-------------|
| `script_reads_written_value` | Attach a script, write a value — verify script received the value |
| `script_triggers_cascading_write` | Script calls write to another path — verify second path has the value |
| `script_error_does_not_block_write` | Script with syntax error — verify the original write still succeeds |

---

## Priority Order

1. **`engine_integration.rs`** — highest value, covers the main gap (cross-module wiring)
2. **`subscription_lifecycle.rs`** — second highest, multi-step stateful scenarios are error-prone
3. **`http_routing.rs`** — medium, mostly thin wiring but catches URI↔path translation bugs
4. **`scripting_integration.rs`** — lower priority, scripting is a later feature

---

## Running

```bash
# All integration tests (+ unit tests)
cargo test-host

# Single integration test file
cargo test-host --test engine_integration
cargo test-host --test subscription_lifecycle
cargo test-host --test http_routing
cargo test-host --test scripting_integration
```

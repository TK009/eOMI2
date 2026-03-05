# OMI-Lite Implementation Plan

Implementation of the OMI-Lite protocol (v1.0) for ESP32-S2 in Rust.

**Current state**: Wi-Fi + basic HTTP server (GET /, POST /) on port 80. Stub modules for http, html, js, device. No data model, no protocol handling.

**Target state**: Full OMI-Lite server — hierarchical object tree, JSON message handling, REST discovery, subscriptions, WebSocket support.

---

## Phase 1: Core Data Model (`src/odf/`)

Platform-independent. Fully testable on host (no ESP32 needed).

### 1.1 Value type and timestamp

- Define `Value { v: OmiValue, t: f64 }` where `OmiValue` is an enum over JSON types (Number(f64), String, Bool, Null)
- Unix timestamp as f64 (fractional seconds)
- Implement `Ord` by timestamp for sorting

### 1.2 Circular value buffer

- Fixed-capacity ring buffer for InfoItem value history
- Configurable max depth per item (default 100 for sensors, 10 for controls)
- Support queries: `newest(n)`, `oldest(n)`, `range(begin, end)`
- Combined queries: time range first, then count limit
- Memory-efficient: no heap allocation beyond initial capacity

### 1.3 InfoItem

- Fields: `type_uri: Option<String>`, `desc: Option<String>`, `meta: BTreeMap<String, OmiValue>`, `values: RingBuffer<Value>`
- Metadata is flat key-value (unit, accuracy, writable, readable, format, latency)
- `is_writable()` checks `meta["writable"]`, default false
- `add_value(v, t)` appends to ring buffer

### 1.4 Object

- Fields: `id: String`, `type_uri: Option<String>`, `desc: Option<String>`, `items: BTreeMap<String, InfoItem>`, `objects: BTreeMap<String, Object>`
- Recursive nesting of child Objects

### 1.5 Object tree and path resolution

- Root struct holds `BTreeMap<String, Object>` (top-level objects)
- Path resolution: parse `/Obj/SubObj/Item` → walk tree
- `get_by_path(path) -> Result<PathTarget>` where `PathTarget` is enum { Root, Object, InfoItem }
- `get_by_path_mut(path)` for writes
- `create_path(path)` creates missing intermediate Objects
- `delete_path(path)` removes node and all children; forbid deleting `/`
- Path validation: reject `..`, empty segments, leading/trailing whitespace

### 1.6 JSON serialization

- Serialize Object tree to OMI-Lite JSON format (Section 4 of spec)
- Serialize with `depth` limit support
- Deserialize Object tree from JSON write payloads
- Use `serde_json` with `#[serde(skip_serializing_if)]` for optional fields
- Keep serialization separate from data model (traits/impls in own file)

### 1.7 Tests

- Unit tests for all of the above, runnable on host with `cargo test`
- Path resolution: valid paths, invalid paths, edge cases
- Ring buffer: overflow, queries, empty buffer
- Serialization round-trips
- Tree mutation: create, read, update, delete

**Dependencies to add**: `serde`, `serde_json` (with `no_std`-compatible features if possible)

---

## Phase 2: Message Parsing & Response Building (`src/omi/`)

Platform-independent. Fully testable on host.

### 2.1 Message envelope

- Parse incoming JSON: `{ "omi": "1.0", "ttl": N, "<op>": { ... } }`
- Validate: exactly one operation, `omi` must be `"1.0"`, `ttl` is required
- Represent as `OmiMessage { version, ttl, operation: Operation }`
- `Operation` enum: `Read`, `Write`, `Delete`, `Cancel`, `Response`

### 2.2 Read operation

- Parse fields: `path`, `rid`, `newest`, `oldest`, `begin`, `end`, `depth`, `interval`, `callback`
- Validate mutual exclusivity: `path` xor `rid`
- Distinguish: one-time read vs subscription (has `interval`) vs poll (has `rid`)

### 2.3 Write operation

- Parse three forms, validate mutual exclusivity:
  - Single value: `path` + `v` (+ optional `t`)
  - Batch: `items` array of `{path, v, t?}`
  - Object tree: `path` + `objects`

### 2.4 Delete operation

- Parse `path`, validate not `/`

### 2.5 Cancel operation

- Parse `rid` array of strings

### 2.6 Response builder

- Build response envelope: `{ "omi": "1.0", "ttl": 0, "response": { "status", "rid?", "desc?", "result?" } }`
- Helper functions for common responses: `ok()`, `created()`, `not_found(path)`, `forbidden(desc)`, `bad_request(desc)`, `error(desc)`
- Partial success builder for batch writes

### 2.7 Tests

- Parse valid messages for each operation type
- Reject malformed messages (missing omi, bad ttl, multiple ops, etc.)
- Response serialization
- Round-trip: parse → process → serialize response

---

## Phase 3: Request Processing Engine (`src/omi/engine.rs`)

Platform-independent core logic. Fully testable on host.

### 3.1 Engine struct

- Holds: `ObjectTree`, subscription registry, request ID generator
- `fn process(&mut self, msg: OmiMessage) -> OmiResponse`
- TTL checking: if ttl=0, must respond immediately; track elapsed time

### 3.2 Read processing

- One-time read: resolve path → serialize node with query params (newest/oldest/begin/end/depth)
- Return 404 if path not found
- Subscription read: validate interval, register subscription, return rid
- Poll read: look up rid, return accumulated values since last poll

### 3.3 Write processing

- Single value: resolve/create path → append value → trigger subscriptions → return 200/201
- Batch: process each item, collect per-item status, return aggregate
- Object tree: recursively create/update structure
- Check `writable` metadata before writing to existing InfoItems → 403 if not writable

### 3.4 Delete processing

- Resolve path → remove from tree → notify affected subscriptions → return 200
- Return 403 for `/`, 404 for nonexistent path

### 3.5 Cancel processing

- Look up each rid → remove subscription → return 200
- Silently skip unknown rids (idempotent)

### 3.6 Tests

- End-to-end: message in → process → response out
- Full scenarios from spec Section 9 (examples 9.1–9.12)
- Edge cases: write to read-only, delete root, cancel nonexistent, expired TTL

---

## Phase 4: Subscription System (`src/omi/subscriptions.rs`)

Platform-independent core, but delivery mechanism is platform-specific.

### 4.1 Subscription registry

- `Subscription { rid, path, interval, callback, created_at, ttl, last_poll }`
- Store in `BTreeMap<String, Subscription>` keyed by rid
- Also maintain path → [rid] index for efficient lookup on value change

### 4.2 Event-based subscriptions (interval = -1)

- On write to a path, check path→rid index
- For each matching subscription, queue the new value for delivery
- Delivery targets: callback URL, WebSocket connection, or poll buffer

### 4.3 Interval-based subscriptions (interval > 0)

- Periodic timer checks which subscriptions need delivery
- Read current value at subscription's path, deliver it
- Platform layer provides the timer tick; engine just processes "check subscriptions now"

### 4.4 Subscription lifetime

- Check TTL on each delivery attempt; expire if past `created_at + ttl`
- Cancel via cancel operation
- WebSocket disconnect cancels WS-bound subscriptions

### 4.5 Poll buffer

- For HTTP subscriptions without callback: buffer values since last poll
- `poll(rid)` returns buffered values and clears buffer
- Bounded buffer size to prevent memory exhaustion

### 4.6 Tests

- Create/cancel/expire subscriptions
- Event delivery triggers on write
- Interval delivery scheduling
- Poll accumulation and retrieval
- TTL expiry
- Multiple subscriptions on same path

---

## Phase 5: HTTP Server Integration (`src/http/`)

ESP32-specific. Uses `esp-idf-svc` HTTP server.

### 5.1 Refactor existing HTTP server

- Move HTTP handler code from `main.rs` to `src/http/`
- Keep `main.rs` for Wi-Fi init and server startup only

### 5.2 POST /omi endpoint

- Accept `application/json` body
- Parse as OmiMessage → pass to engine → serialize response
- Return JSON response with appropriate HTTP status
- Handle transport errors: 400 for unparseable JSON, 405 for wrong method

### 5.3 GET /omi/* REST discovery

- Route: `GET /omi/`, `GET /omi/{path...}/`, `GET /omi/{path...}/{item}`
- Trailing slash = object listing, no trailing slash = InfoItem value
- Parse query params: `newest`, `oldest`, `begin`, `end`, `depth`
- Translate to internal read operation → engine → response
- Return JSON (same format as POST responses)

### 5.4 Callback delivery (HTTP client)

- For subscriptions with callback URL: POST the response JSON to that URL
- Use `esp-idf-svc` HTTP client or `esp_idf_svc::http::client`
- Fire-and-forget with retry on failure (1 retry, then drop)
- Run delivery in separate task/thread to not block the server

### 5.5 Content negotiation

- Check `Content-Type` and `Accept` headers
- JSON is the only required encoding; CBOR is stretch goal
- Return 415 Unsupported Media Type for unknown content types

### 5.6 Tests

- Integration tests with HTTP client on device (or mock in host tests)
- Verify all REST paths return correct structure
- Verify POST processing end-to-end

---

## Phase 6: WebSocket Support (`src/ws/`)

ESP32-specific. Depends on ESP-IDF WebSocket capabilities.

### 6.1 WebSocket endpoint

- Upgrade endpoint at `/omi/ws`
- Use `esp-idf-svc` WebSocket support or raw ESP-IDF WebSocket API
- Track active WebSocket connections

### 6.2 Message handling over WebSocket

- Receive JSON messages → parse as OmiMessage → process → send response
- Same logic as POST /omi, different transport

### 6.3 Subscription delivery over WebSocket

- Subscriptions created over WS without callback → deliver on same WS
- Associate subscription rid with WS connection
- On WS disconnect → cancel all associated subscriptions

### 6.4 Tests

- WebSocket connect/disconnect
- Message exchange over WS
- Subscription delivery over WS

---

## Phase 7: Persistence & Device Init

### 7.1 Object tree initialization

- On boot, populate the tree with the device's own sensors/actuators
- E.g., DHT11 → `/Dht11/Temperature`, `/Dht11/RelativeHumidity`
- Platform-specific sensor drivers write values into the tree periodically

### 7.2 Optional: NVS persistence

- Store tree structure in ESP32 NVS (Non-Volatile Storage) for survival across reboots
- Or regenerate from code on each boot (simpler, less flash wear)

---

## Phase 8: Script Engine Integration

### 8.1 On-write triggers

- InfoItems can have an associated script (stored in metadata or separate field)
- When a value is written, execute the script with `event.value` context
- Script can call `odf.writeItem(value, path)` to trigger cascading writes
- This replaces the O-MI `call` operation

### 8.2 JavaScript engine (QuickJS)

- Integrate QuickJS via C FFI as ESP-IDF component
- Provide `global` object for persistent state across invocations
- Provide `event` object with current write context
- Provide `odf.writeItem(value, path)` binding

---

## Implementation Order & Dependencies

```
Phase 1 ──→ Phase 2 ──→ Phase 3 ──→ Phase 5 (HTTP)
                │                       │
                │                  Phase 6 (WebSocket)
                │
                └──→ Phase 4 ──→ (integrates into Phase 3)

Phase 7 (device init) depends on Phase 3
Phase 8 (scripts) depends on Phase 3 + Phase 7
```

**Host-testable (no hardware)**: Phases 1–4
**Requires ESP32**: Phases 5–8

---

## Crate Dependencies to Add

| Crate | Purpose | Phase |
|-------|---------|-------|
| `serde` | Serialization framework | 1 |
| `serde_json` | JSON parsing/generation | 1 |
| `heapless` | Fixed-size collections (optional, for no_std ring buffer) | 1 |

No external crates needed for Phases 2–4 beyond serde.
ESP-IDF crates already present cover Phases 5–6.

---

## Key Design Decisions

1. **BTreeMap over HashMap** — deterministic ordering, no hasher dependency, friendlier to constrained memory allocators.

2. **Ring buffer for values** — bounded memory, O(1) insert, supports newest/oldest queries naturally.

3. **Engine is transport-agnostic** — processes `OmiMessage → OmiResponse`. HTTP and WebSocket layers just serialize/deserialize and call the engine. This keeps the core testable on host.

4. **No CBOR in first pass** — JSON only. CBOR can be added later as an alternative serializer over the same data model.

5. **No `call` operation** — per spec, use write + on-write scripts. This is Phase 8.

6. **Subscriptions use String rids** — simple incrementing counter formatted as string (`"sub-001"`, `"sub-002"`).

7. **Platform separation** — `src/odf/` and `src/omi/` are platform-independent. `src/http/` and `src/ws/` are ESP32-specific. Matches CLAUDE.md conventions.

# Feature Specification: JavaScript Callback Subscriptions

**Feature Branch**: `008-javascript-callback-subscriptions`
**Created**: 2026-03-15
**Status**: Draft
**Input**: Thesis-described `javascript://` callback URL protocol for running scripts on intervals via local O-MI subscriptions

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Periodic Script Execution via Interval Subscription (Priority: P1)

A device operator wants a script to run every N seconds — for example, a controller that checks a sensor reading and adjusts an actuator. They create a MetaData InfoItem containing the script text, then create an O-MI interval subscription with `callback="javascript://Objects/MyDevice/Controller/MetaData/controlScript"` and `interval="10"`. Every 10 seconds, the subscription tick fires, collects the subscribed values, and passes them to the script at that MetaData InfoItem. The script runs with the subscription results as its `event` argument (an array of `{value, path, timestamp}` objects) and can call `odf.writeItem()` to actuate.

**Why this priority**: This is the core feature — periodic autonomous script execution on the device with no external network calls. Without it, scripts can only react to external writes or reads, not run proactively.

**Independent Test**: Create a MetaData InfoItem with script text as its value, create an interval subscription pointing `javascript://` at that MetaData item, advance time past the interval, and verify the script executed (e.g. by checking a value it wrote via `odf.writeItem()`).

**Acceptance Scenarios**:

1. **Given** an Object `Objects/Heater` with a MetaData InfoItem `controlScript` whose value is JS that reads `event.values[0].value` (temperature) and writes to the switch, and an interval subscription on `Objects/Heater/Temperature` with `callback="javascript://Objects/Heater/MetaData/controlScript"` and `interval="10"`, **When** 10 seconds elapse and the subscription tick fires, **Then** the script executes with `event.values` containing `[{value: <temp>, path: "Objects/Heater/Temperature", timestamp: <ts>}]`, and the switch value is updated based on the script logic.
2. **Given** the same setup with `ttl="3600"`, **When** 3601 seconds elapse, **Then** the subscription expires and the script stops executing periodically.
3. **Given** a `javascript://` subscription with `interval="5"`, **When** the tick fires, **Then** no HTTP request is made — delivery is entirely internal.

---

### User Story 2 - Multi-Item Monitoring Script (Priority: P2)

A device operator wants a script that monitors several sensors and computes a combined status. They create a subscription on an Object (subtree) with a `javascript://` callback. On each interval tick, the subscription collects all InfoItem values under the subscribed Object and passes them to the target script as an array of `{value, path, timestamp}` entries.

**Why this priority**: Enables richer automation where a single script reacts to a collection of values, not just one InfoItem. Depends on the basic `javascript://` routing from P1.

**Independent Test**: Subscribe to an Object containing multiple InfoItems with a `javascript://` callback. Advance time, verify the script receives `event.values` containing entries for all subscribed items, and that it can compute an aggregate result.

**Acceptance Scenarios**:

1. **Given** an Object `Objects/Room1` containing InfoItems `Temperature`, `Humidity`, and `CO2`, and a `javascript://` subscription on `Objects/Room1` with `interval="30"`, **When** the interval fires, **Then** the target script receives `event.values` as `[{value: 22.5, path: "Objects/Room1/Temperature", timestamp: ...}, {value: 65, path: "Objects/Room1/Humidity", timestamp: ...}, {value: 400, path: "Objects/Room1/CO2", timestamp: ...}]` and can write a computed `AirQuality` score.
2. **Given** the same setup, **When** one InfoItem has no stored value (empty ring buffer), **Then** that item's entry in `event.values` has `value: null` and the script still executes.

---

### User Story 3 - Event-Driven Script Callback (Priority: P3)

A device operator wants a script to run whenever a specific value changes, not on a fixed interval. They create an event subscription (`interval="-1"`) with a `javascript://` callback. When any write updates the subscribed path, the subscription fires and the target script executes with the new value.

**Why this priority**: Event-driven callbacks complement interval-driven ones. Useful for immediate reactions (e.g. alarm triggers) without polling overhead. Lower priority because interval subscriptions cover most automation use cases.

**Independent Test**: Create an event subscription with `javascript://` callback, write a new value to the subscribed path, verify the target script executes with the written value.

**Acceptance Scenarios**:

1. **Given** an event subscription on `Objects/DoorSensor/State` with `callback="javascript://Objects/Alarm/MetaData/checkDoor"`, **When** a value is written to `Objects/DoorSensor/State`, **Then** the `checkDoor` script executes with the new value as `event`.
2. **Given** the same setup, **When** no write occurs, **Then** the script does not execute (no spurious triggers).

---

### Edge Cases

- What happens when the `javascript://` path does not resolve to a valid MetaData InfoItem? The delivery is dropped and a warning is logged. The subscription remains active (the script may be written later).
- What happens when the `javascript://` path points to a MetaData InfoItem that exists but has no value (empty)? Same as above — delivery dropped, warning logged.
- What happens when the `javascript://` path points to a regular InfoItem (not in MetaData)? Delivery is dropped with a warning. Scripts for `javascript://` callbacks MUST reside in MetaData InfoItems only.
- What happens when the target script errors or times out? The error is logged, the subscription remains active, and the next interval fires normally. Script errors never cancel subscriptions.
- What happens when a `javascript://` callback script calls `odf.writeItem()` on a path that has an `onwrite` script? The `onwrite` script executes normally — script chaining is supported, subject to the existing depth limit.
- What happens when a `javascript://` callback script calls `odf.writeItem()` on a path that itself has a `javascript://` event subscription? The event subscription fires, potentially creating a chain. The existing script depth limit prevents infinite recursion.
- What happens when the `javascript://` URL is malformed (no path, invalid path syntax)? The subscription is created (the URL is opaque to the subscription registry), but delivery fails at dispatch time with a logged warning.
- What happens when the subscribed path has an `onread` script? For interval `javascript://` callback deliveries, onread scripts run on the subscribed values before they reach the callback script (consistent with existing interval subscription behavior per spec 006 FR-002). For event `javascript://` deliveries, onread does NOT run (consistent with spec 006).
- What happens when a `javascript://` subscription and an HTTP/WS subscription exist for the same path and interval? Both fire independently — the `javascript://` one routes internally, the HTTP/WS one routes externally.
- What happens when the device reboots? Subscriptions are in-memory only and do not survive reboot. The operator must re-create them (consistent with existing subscription behavior).
- What happens when the subscribed path and the script path are on the same Object (e.g. subscribe to `Objects/Sensor/Temp`, callback to `Objects/Sensor/Temp/MetaData/autoControl`)? This is allowed and expected — a common pattern for self-monitoring items.
- What happens with the callback script's return value? It is ignored. Callback scripts act purely through `odf.writeItem()` side effects.
- Can MetaData InfoItems have `onread` or `onwrite` scripts? No. MetaData InfoItems are data containers only — they do not support script triggers. The `javascript://` target is read as a plain value (the script text), with no trigger side effects.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST recognize callback URLs with the `javascript://` protocol scheme in subscription creation requests. The URL format is `javascript://<O-DF-path>` where `<O-DF-path>` is a slash-separated path to a MetaData InfoItem whose stored value is the script text (e.g. `javascript://Objects/Device/Controller/MetaData/myScript`). The target path MUST point to a MetaData InfoItem; paths to regular InfoItems are not valid targets.
- **FR-002**: When a `javascript://` callback subscription fires (interval tick or event notification), the system MUST resolve the O-DF path from the URL, read the script text from the target MetaData InfoItem's most recent value, and execute it via the script engine. The subscription result values MUST be passed to the script as the `event` argument. Any client (local or remote) MAY create subscriptions with `javascript://` callbacks via the standard O-MI protocol.
- **FR-003**: The `javascript://` delivery MUST NOT generate any network traffic — it is an entirely internal routing mechanism.
- **FR-004**: The `javascript://` callback script MUST have access to both `odf.readItem()` and `odf.writeItem()`. Unlike `onread` scripts (which are side-effect-free), callback scripts are intended to perform actions.
- **FR-005**: The `javascript://` callback script MUST be subject to the same resource limits (time limit, operation limit) and depth limit as `onwrite` scripts.
- **FR-006**: If the `javascript://` path does not resolve to a valid script (path not found, no MetaData script at target, empty value), the system MUST log a warning and drop the delivery. The subscription MUST remain active.
- **FR-007**: If the `javascript://` callback script errors, times out, or exceeds resource limits, the system MUST log a warning. The subscription MUST remain active and fire again at the next interval.
- **FR-008**: `javascript://` callback subscriptions MUST be subject to the same TTL and expiry rules as all other subscriptions.
- **FR-009**: `javascript://` callback subscriptions MUST count toward the existing `MAX_SUBSCRIPTIONS` limit.
- **FR-010**: For interval `javascript://` subscriptions, `onread` scripts on the subscribed InfoItems MUST execute before the values are passed to the callback script (consistent with spec 006 FR-002).
- **FR-011**: For event `javascript://` subscriptions, `onread` scripts MUST NOT execute — event delivery passes the written value as-is (consistent with spec 006).
- **FR-012**: Script chaining via `odf.writeItem()` from a `javascript://` callback MUST trigger `onwrite` scripts and event subscriptions on the written path, subject to the existing script depth limit.
- **FR-013**: The `javascript://` callback script's return value MUST be ignored. Callback scripts produce effects solely via `odf.writeItem()` and `odf.readItem()`.
- **FR-014**: The `javascript://` path MUST resolve to a MetaData InfoItem. If it resolves to a regular (non-MetaData) InfoItem, the delivery MUST be dropped with a logged warning.

### Key Entities

- **DeliveryTarget**: Extended with a `Script(String)` variant (or recognized as a sub-case of `Callback`) where the string is the O-DF path extracted from the `javascript://` URL.
- **Subscription**: No structural changes — the `javascript://` URL is stored in the existing `Callback(String)` target. Routing is determined at delivery time by inspecting the URL scheme.
- **Event object for callback scripts**: `{ values: [{value, path, timestamp}, ...] }` — an array of value entries from the subscription result. Each entry contains the value (or `null` if ring buffer empty), the O-DF path of the source InfoItem, and the timestamp. For single-InfoItem subscriptions, the array has one element. For Object (subtree) subscriptions, it contains one entry per InfoItem under the subscribed path.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A `javascript://` interval subscription executes its target script on every interval tick within the existing script execution time budget.
- **SC-002**: A `javascript://` event subscription executes its target script on every write to the subscribed path within the existing script execution time budget.
- **SC-003**: No HTTP/network traffic is generated by `javascript://` callback deliveries.
- **SC-004**: Script errors in `javascript://` callbacks never cancel the parent subscription — 100% of subsequent ticks still fire.
- **SC-005**: `javascript://` subscriptions have no measurable impact on the tick performance of non-script subscriptions.
- **SC-006**: Existing subscription tests continue to pass unmodified — `javascript://` is additive.
- **SC-007**: A `javascript://` callback script can successfully chain to other scripts via `odf.writeItem()`, with depth limiting preventing infinite recursion.

## Constitution Compliance

- **I. Resource Efficiency**: No new scheduling infrastructure — reuses the existing subscription queue and tick mechanism. The `javascript://` URL is parsed at delivery time with a simple `starts_with` check, adding negligible CPU. No additional memory allocation beyond what existing callback subscriptions use. Scripts are subject to existing time/operation limits.
- **II. Reliability**: Script errors and missing targets are handled gracefully (log + drop delivery, subscription stays active). Depth limiting prevents recursive script chains. TTL expiry prevents leaked subscriptions.
- **III. Platform Separation**: The `javascript://` routing logic lives in the platform-independent engine/delivery layer (`src/omi/engine.rs`), not in ESP-specific code. The script engine is already platform-independent. The only platform-specific code (`src/callback.rs`) is unchanged — it simply never receives `javascript://` URLs because they're intercepted before reaching HTTP delivery.
- **IV. Test Discipline**: All `javascript://` routing, script execution, error handling, and chaining can be tested on the host without hardware. Integration with the subscription queue reuses existing test patterns from `subscriptions.rs`. E2e tests validate the full loop on device.
- **V. Simplicity**: No new data structures, no new scheduling mechanism, no new script trigger type. The feature is a thin routing layer (`if url.starts_with("javascript://")`) on top of existing subscription delivery + script execution infrastructure.

## Clarifications

### Session 2026-03-15

- Q: What should the `event` object look like for callback scripts — same as onwrite `{value, path, timestamp}` or array-based? → A: Array-based: `{ values: [{value, path, timestamp}, ...] }`. This supports both single-item and multi-item (Object subtree) subscriptions uniformly.
- Q: Where does the `javascript://` path point to — MetaData InfoItem value, or an existing metadata key like onwrite? → A: MetaData InfoItem only. The path must resolve to a MetaData InfoItem whose stored value is the script text. This follows the thesis approach and keeps script storage consistent (scripts are always in MetaData).
- Q: Should the callback script's return value do anything? → A: Ignored. Callback scripts act purely through `odf.writeItem()` side effects.
- Q: How are `javascript://` subscriptions created — restricted to local, or via standard O-MI? → A: Via normal O-MI protocol. Any client (local or remote) can create a subscription with `callback="javascript://..."`. The URL resolves locally on the device — it names a local script, not a remote resource.
- Q: Can the subscribed path and the script path be on the same InfoItem? → A: Yes, allowed. Self-monitoring is a valid and expected pattern.
- Q: Does reading the script text from the target MetaData InfoItem trigger onread? → A: No. MetaData InfoItems cannot have onread or onwrite scripts. They are plain data containers.
- Q: Should we enforce a higher minimum interval for `javascript://` subscriptions given the 5s tick? → A: No. The current 0.1s minimum floor and 5s tick resolution are known system constraints. Fixing tick resolution is a separate task.
- Q: Should `javascript://` deliveries be processed before or after HTTP/WS deliveries? → A: No special ordering. They are processed alongside other deliveries in whatever order the delivery loop iterates.

## Assumptions

- The existing mJS script engine, resource limits, and depth-limiting infrastructure are reused — no new scripting runtime is needed.
- The `event` object for callback scripts uses an array format `{ values: [{value, path, timestamp}, ...] }` rather than the single-item `{ value, path, timestamp }` format used by onwrite/onread. This is a different shape, chosen to support multi-item subscriptions.
- Scripts for `javascript://` callbacks MUST be stored in MetaData InfoItems (their stored value is the script text). This is distinct from `onwrite`/`onread` scripts which are stored as metadata keys on regular InfoItems.
- Subscriptions are in-memory only and do not persist across reboots. If persistent scheduled scripts are needed, that is a separate feature (NVS-backed subscription restore).
- The `javascript://` scheme resolves locally on the device processing the subscription. The URL names a local MetaData InfoItem path, not a remote resource. However, any O-MI client (including remote ones) can create such subscriptions.
- The 5-second main loop tick resolution applies to `javascript://` interval subscriptions just as it does to all other interval subscriptions. Sub-5-second intervals are accepted but fire at the next available tick. Improving tick resolution is out of scope for this spec.

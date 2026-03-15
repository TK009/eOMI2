# Implementation Plan: InfoItem OnRead Script Trigger

**Spec**: [spec.md](./spec.md)
**Branch**: `006-infoitem-onread-trigger`
**Created**: 2026-03-07

## Architecture Overview

The `onread` feature mirrors the existing `onwrite` infrastructure. When an
InfoItem with an `onread` metadata key is read, the system executes the script
and delivers the script's return value instead of the stored value. The stored
value is never mutated.

### Execution points (3 read paths):
1. **Direct reads** — `process_read_one_time()` in `engine.rs`
2. **Interval subscription delivery** — `tick()` in `engine.rs`
3. **Nested script reads** — `odf.readItem()` callback in `bindings.rs`

Event-based subscriptions are explicitly excluded (deliver written value as-is).

---

## Tasks

### Task 1: InfoItem `onread` metadata accessor
**Priority**: P1 | **Estimate**: 30min | **Dependencies**: none

Add `get_onread_script() -> Option<&str>` helper to `InfoItem` in
`src/odf/item.rs`. Reads the `"onread"` key from `meta`. Symmetrical with how
`onwrite` is accessed in `engine.rs` today (inline meta lookup), but extracted
as a reusable helper.

**Files**: `src/odf/item.rs`

**Acceptance**: Unit test that an InfoItem with `meta: {"onread": "event.value * 2"}`
returns `Some("event.value * 2")` and one without returns `None`.

---

### Task 2: `run_onread_script()` core function
**Priority**: P1 | **Estimate**: 2h | **Dependencies**: Task 1

Create `run_onread_script()` in `src/omi/engine.rs` following the
`run_onwrite_script()` pattern. Key differences from onwrite:

- **Input**: path, stored value, stored timestamp, depth
- **Output**: `Option<OmiValue>` — the transformed value, or `None` on
  error/undefined (caller falls back to stored value)
- **Event object**: `{ value, path, timestamp }` — same structure as onwrite
- **Bindings**: `odf.readItem()` only — NO `odf.writeItem()` (FR-006)
- **Resource limits**: Same constants (`MAX_SCRIPT_OPS`, `MAX_SCRIPT_EXEC_MS`,
  `MAX_SCRIPT_DEPTH`) (FR-010)
- **Error handling**: Log warning via `script_rl`, return `None` (FR-005)
- **Return value extraction**: Convert mJS result back to `OmiValue` via
  `mjs_to_omi()`. If result is `undefined`/`null`, return `None`.

**Files**: `src/omi/engine.rs`, `src/scripting/bindings.rs`

**Acceptance**: Integration test: InfoItem with `onread: "event.value * 0.01 - 40"`,
stored value `6500`, read returns `25.0`. Script error returns stored value.

---

### Task 3: Self-read recursion guard
**Priority**: P1 | **Estimate**: 1h | **Dependencies**: Task 2

Prevent infinite recursion when an `onread` script calls `odf.readItem()` on
its own path (FR-008). Track the "currently executing onread path" in
`ScriptCallbackCtx` (or a new field). When `odf.readItem()` detects a
self-referencing read, return the stored value directly without triggering the
script.

Also ensure cascading reads (item A's onread reads item B which has its own
onread) work correctly subject to depth limits (FR-007).

**Files**: `src/scripting/bindings.rs`, `src/omi/engine.rs`

**Acceptance**: Test that `odf.readItem("/self")` inside `/self`'s onread
returns stored value. Test cascading A→B works. Test depth limit exceeded
returns stored value.

---

### Task 4: Integrate onread into direct read path
**Priority**: P1 | **Estimate**: 1h | **Dependencies**: Task 2

Modify `process_read_one_time()` in `engine.rs` to:
1. After `query_values()`, check if item has an `onread` script
2. If yes, call `run_onread_script()` with the newest value
3. Replace only the newest value in the response; older values returned as-is
   (FR-012)
4. Preserve original timestamps (FR-011)
5. If script returns `None`, use stored value unchanged

Handle edge case: empty ring buffer → pass `null` as `event.value` (spec edge
case).

**Files**: `src/omi/engine.rs`

**Acceptance**: End-to-end test: write raw value, read with onread script,
verify transformed value returned. Verify `newest=5` only transforms newest.

---

### Task 5: Integrate onread into interval subscription delivery
**Priority**: P2 | **Estimate**: 1h | **Dependencies**: Task 4

Modify `tick()` in `engine.rs` to run `onread` scripts for interval
subscription values before delivery. The closure passed to
`tick_intervals()` currently reads raw values — extend it to also apply
onread transformation.

Event-based subscriptions (`notify_write_event()`) must NOT run onread
(FR-002). Verify this is already the case since event delivery uses write
values.

**Files**: `src/omi/engine.rs`

**Acceptance**: Test: interval subscription on item with onread script delivers
transformed value. Event subscription delivers raw written value.

---

### Task 6: Integrate onread into `odf.readItem()` binding
**Priority**: P2 | **Estimate**: 1.5h | **Dependencies**: Task 3

When a script calls `odf.readItem("/path")` and the target item has an
`onread` script, the nested script must execute (FR-007), subject to depth
limits. Modify `js_odf_read_item()` in `bindings.rs` to:

1. Check if resolved item has an `onread` script
2. If yes, and not self-referencing, and within depth limit: execute nested
   onread script
3. Return the transformed value

This requires access to the script engine from within the callback context.
The `ScriptCallbackCtx` needs to carry enough state to run nested scripts
(or the nested execution must be deferred like pending writes).

**Files**: `src/scripting/bindings.rs`, `src/omi/engine.rs`

**Acceptance**: Test: script A reads item B via `odf.readItem()`, item B has
onread script, returned value is B's transformed value.

---

### Task 7: Onwrite + onread independence
**Priority**: P2 | **Estimate**: 30min | **Dependencies**: Task 4

Verify and test that `onwrite` and `onread` scripts on the same InfoItem
operate independently (FR-009). Write triggers onwrite, read triggers onread,
no interference.

**Files**: `tests/scripting_integration.rs`

**Acceptance**: Test: item with both scripts, write triggers only onwrite,
read triggers only onread, values correct.

---

### Task 8: Host test suite
**Priority**: P1 | **Estimate**: 2h | **Dependencies**: Tasks 4, 5, 6, 7

Comprehensive `cargo test-host` tests covering all FRs and edge cases:

- FR-001: onread metadata key parsed correctly
- FR-002: read/interval/readItem trigger; event does not
- FR-003: event object has {value, path, timestamp}
- FR-004: stored value unchanged after onread
- FR-005: script error → stored value returned, warning logged
- FR-006: odf.writeItem() not available in onread
- FR-007: cascading onread across items
- FR-008: self-read returns stored value
- FR-009: onwrite + onread independence
- FR-010: resource limits enforced
- FR-011: timestamps preserved in response
- FR-012: only newest value transformed

**Files**: `tests/scripting_integration.rs` (extend existing), potentially new
`tests/onread_integration.rs`

**Acceptance**: All tests pass with `cargo test-host`.

---

### Task 9: E2E device tests
**Priority**: P3 | **Estimate**: 1.5h | **Dependencies**: Task 8

Add e2e tests in `tests/e2e/` that exercise onread on real hardware:
- HTTP read of item with onread script
- WebSocket subscription with onread
- Verify computed values over the wire

**Files**: `tests/e2e/test_onread.py`

**Acceptance**: `./scripts/run-e2e.sh` passes with new tests.

---

## Dependency Graph

```
Task 1 (metadata accessor)
  └─► Task 2 (core run_onread_script)
        ├─► Task 3 (self-read guard)
        │     └─► Task 6 (odf.readItem integration)
        ├─► Task 4 (direct read path)
        │     ├─► Task 5 (interval subscription)
        │     ├─► Task 7 (onwrite+onread independence)
        │     └─► Task 8 (host tests) ◄── also depends on 5, 6, 7
        └─────────────────────────────► Task 8
              Task 8
                └─► Task 9 (e2e tests)
```

## Implementation Order (suggested)

1. Task 1 → Task 2 → Task 3 (foundation)
2. Task 4 → Task 5 (read paths)
3. Task 6 (nested reads — can parallel with 4+5 after Task 3)
4. Task 7 (quick verification)
5. Task 8 (comprehensive tests)
6. Task 9 (hardware tests — last)

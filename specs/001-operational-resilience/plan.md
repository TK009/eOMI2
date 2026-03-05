# Implementation Plan: Operational Resilience Improvements

**Branch**: `001-operational-resilience` | **Date**: 2026-03-05 | **Spec**: [spec.md](spec.md)

## Summary

Add wall-clock time limit for mJS script execution (alongside existing op-count limit) and structured logging with per-module levels, session IDs, and rate-limiting. These address the two highest operational gaps from the architecture review: device hangs from bad scripts (P1) and poor field observability (P2).

## Technical Context

**Language/Version**: Rust 2021 edition, ESP-IDF toolchain
**Primary Dependencies**: mJS (C, vendored), esp-idf-svc 0.51, log 0.4, serde/serde_json
**Storage**: NVS (ESP-IDF non-volatile storage)
**Testing**: `cargo test` (host-only), `run-e2e.sh` (hardware)
**Target Platform**: ESP32 (Xtensa), embedded Linux for host tests
**Constraints**: ~16KB HTTP thread stack, PSRAM available, `panic = "abort"` in release

## Architecture Decisions

### AD-1: Wall-clock timeout via `std::time::Instant` polling in mJS op callback

mJS has no built-in wall-clock timeout. The options are:

1. **FreeRTOS watchdog task** — spawn a timer task that kills the script thread on timeout. Complex, requires cross-thread signaling, risky on ESP32 single-core.
2. **mJS op callback with `Instant` check** — mJS calls a user-defined C callback on each bytecode op (same mechanism as op-count limit). We check `Instant::elapsed()` inside it. Zero extra threads, minimal overhead (one clock read per N ops, not every op).
3. **Separate thread with `std::thread::spawn`** — not viable on ESP32 single-core with the current synchronous server model.

**Decision**: Option 2. The mJS `set_ops_cb` callback is called on every op tick. We'll check wall-clock time every `CHECK_INTERVAL_OPS` operations (e.g., every 1000 ops) to amortize the cost of `Instant::now()`. When the deadline is exceeded, we set an error flag that causes mJS to abort. This piggybacks on the existing op-count infrastructure.

**Key insight**: mJS already has `mjs_set_max_ops` and the op-limit error path. We need to expose `mjs_set_ops_cb` in FFI, install a callback that checks both the op count and a wall-clock deadline, and return the appropriate error.

### AD-2: Timeout reporting as partial-success response

Per spec FR-005: when a script times out, the triggering write is already committed. The response must indicate "write succeeded, script failed."

**Decision**: Add a new `OmiResponse::partial_success` variant that carries both a 200 status for the write and a script error description. The existing `ItemStatus` batch mechanism could work, but a simpler approach is to add an optional `warning` field to the response body indicating the script failure. This keeps backwards compatibility — clients that don't understand `warning` still see a 200.

**Revised**: Actually, looking at the existing code flow in `write_single_inner`, script errors are already silently swallowed (logged but not propagated). To satisfy FR-005, we need to thread the script error result back up through `write_single_inner` → `process_write` → `process()` and include it in the response. The simplest change: `run_onwrite_script` returns an `Option<String>` error alongside the deliveries, and `write_single_inner` propagates it.

### AD-3: Structured logging — `esp_idf_svc::log::set_target_level` + manual session context

ESP-IDF's logging already supports per-module (per-"tag") log levels via `esp_log_level_set`. The Rust wrapper `esp_idf_svc::log::set_target_level` is already used in `main.rs` for wifi/httpd. We extend this pattern.

For session IDs in log messages: there's no Rust equivalent of structured logging (like `tracing`) that works well on ESP-IDF. The pragmatic approach is to include session IDs as formatted fields in log message strings: `info!("WS msg: session={} path={}", sid, path)`. This is already partially done in `server.rs`.

For rate-limiting: a simple `(last_msg_hash, last_time, count)` struct that deduplicates repeated identical messages within a configurable window.

### AD-4: No mJS source changes needed

Looking at the mJS source, `mjs_set_max_ops` already sets a limit and `MJS_OP_LIMIT_ERROR` is returned. For the wall-clock timeout, we can either:
- Add a C-level callback via `mjs_set_ops_cb` (if it exists in vendored mJS)
- Or simply check time *before* each `mjs_exec` call and add a wrapper

**Need to verify**: whether vendored mJS exposes an ops callback hook. If not, the simplest approach is wrapping the execution: since scripts are short (max 4KB, max 50K ops), we can check time before/after and rely on the op-count limit to bound execution within a reasonable wall-clock window. Then add a post-execution elapsed-time check and return `TimeLimitExceeded` if it took too long.

**Final decision**: Use a two-phase approach:
1. Op-count limit prevents infinite loops (existing)
2. Post-execution elapsed-time check catches expensive FFI calls
3. If elapsed > `MAX_SCRIPT_EXEC_MS`, log and report error

This is simpler than an in-flight callback and covers the stated threat model (FFI calls that bypass op counter). A true in-flight interrupt would require either mJS source modification or a separate thread, both excessive for the current risk.

## Source Code Layout

```text
src/
├── scripting/
│   ├── engine.rs         # MODIFY: Add wall-clock timeout to exec(), add MAX_SCRIPT_EXEC_MS
│   ├── error.rs          # MODIFY: Add TimeLimitExceeded variant
│   ├── mod.rs            # (no change)
│   ├── ffi.rs            # (no change unless ops callback needed)
│   ├── bindings.rs       # (no change)
│   └── convert.rs        # (no change)
├── omi/
│   ├── engine.rs         # MODIFY: Propagate script timeout error in run_onwrite_script,
│   │                     #         return script error info from write_single_inner
│   └── response.rs       # MODIFY: Add partial_success_with_warning() helper
├── server.rs             # MODIFY: Add session IDs to more log messages
├── main.rs               # MODIFY: Add per-module log levels, configure structured logging
├── log_util.rs           # NEW: Rate-limiting log wrapper
└── lib.rs                # MODIFY: Add log_util module
```

No new dependencies. No new feature flags.

## Tasks

### Phase 1: Script Execution Safety (User Story 1 — P1)

**Goal**: Scripts that exceed wall-clock time are terminated, write is preserved, error reported.

- [ ] T001 [US1] Add `TimeLimitExceeded` variant to `ScriptError` in `src/scripting/error.rs`
  - Add variant with elapsed duration
  - Update `Display` impl

- [ ] T002 [US1] Add wall-clock timeout to `ScriptEngine::exec()` in `src/scripting/engine.rs`
  - Add `MAX_SCRIPT_EXEC_MS` constant (e.g., 500ms — configurable later)
  - Record `Instant::now()` before `mjs_exec()`
  - After execution, check elapsed time
  - If exceeded AND execution succeeded, return `TimeLimitExceeded` (script was slow but completed — still flag it)
  - If execution hit op-limit, that takes precedence
  - Add host test: script that completes but check the timing path works

- [ ] T003 [US1] Propagate script error from `run_onwrite_script()` in `src/omi/engine.rs`
  - Change return type from `Vec<Delivery>` to `(Vec<Delivery>, Option<String>)` where the String is a script error description
  - On `ScriptError::OpLimitExceeded` or `TimeLimitExceeded`, log with path and elapsed time (FR-004), return error string
  - Thread error through `write_single_inner` return type

- [ ] T004 [US1] Add partial-success response for script timeout in `src/omi/response.rs`
  - Add `OmiResponse::write_ok_with_warning(desc: &str)` — status 200 with a `warning` field
  - Or: use `desc` field on the 200 response to carry the script error message

- [ ] T005 [US1] Wire script error into write response path in `src/omi/engine.rs`
  - In `process_write()`, collect script errors from `write_single_inner`
  - If any script errors occurred but writes succeeded, use partial-success response
  - Single writes: 200 with warning desc
  - Batch writes: individual `ItemStatus` entries can carry per-item script warnings

- [ ] T006 [US1] Add tests for script timeout behavior
  - Host unit test: `ScriptEngine::exec()` with a known-duration script, verify timing check path
  - Integration test: write to node with onwrite script that hits op limit, verify response includes script error
  - Integration test: write to node with valid script, verify normal 200 response
  - Integration test: cascading script timeout at depth > 0, verify earlier writes preserved

- [ ] T007 [US1] E2E test: deploy infinite-loop script, verify device stays responsive
  - Write a script `while(true){}` to an onwrite handler
  - Trigger the write
  - Verify error response
  - Verify subsequent writes work normally

### Phase 2: Structured Logging (User Story 2 — P2)

- [ ] T008 [P] [US2] Create `src/log_util.rs` with rate-limiting log wrapper
  - `RateLimiter` struct: tracks `(message_hash, last_emit_time, suppressed_count)`
  - `fn should_emit(msg: &str, window_secs: u64) -> bool`
  - On suppression end, emit "suppressed N messages" summary
  - Configurable window (default 10s per SC-003)
  - Add host unit tests

- [ ] T009 [P] [US2] Add session IDs to WebSocket log messages in `src/server.rs`
  - WS message handler: include `session={}` in all log lines
  - WS connect/disconnect: already has session IDs (verify completeness)
  - Delivery dispatch: include session ID in WS send failure logs

- [ ] T010 [US2] Configure per-module log levels in `src/main.rs`
  - Release builds: set default to `Info`, set `reconfigurable_device::omi` to `Warn`
  - Debug builds: set default to `Debug`
  - Add module-level targets for noisy components
  - Document the log level configuration in code comments

- [ ] T011 [US2] Apply rate-limiting to repeated warnings
  - WiFi reconnect loop in `main.rs` — already partially rate-limited (every 12th), improve with `RateLimiter`
  - Callback delivery failures in `server.rs`
  - Script errors in `omi/engine.rs` (same script failing repeatedly)

- [ ] T012 [US2] Add tests for structured logging
  - Host unit test: `RateLimiter` suppresses repeated messages within window
  - Host unit test: `RateLimiter` emits after window expires
  - Host unit test: different messages are not suppressed

### Phase 3: Polish

- [ ] T013 Verify all acceptance scenarios from spec
  - Walk through each scenario in spec.md and verify coverage
  - Run `cargo test-host` — all pass
  - Run E2E test suite if hardware available

- [ ] T014 Update `tasks/architecture-review.md` to mark issues 1 and 5 as addressed

## Dependencies & Execution Order

```
T001 ──→ T002 ──→ T003 ──→ T005 ──→ T006 ──→ T007
                    ↓
                   T004 ──→ T005

T008 ─────────────────────→ T011
T009 (parallel with T008)
T010 (parallel with T008, T009)
                              ↓
                             T012

T013 depends on all above
T014 depends on T013
```

**Story 1 (P1)** is sequential: error type → engine timeout → propagation → response → wiring → tests.
**Story 2 (P2)** tasks T008/T009/T010 can run in parallel, then T011/T012 depend on T008.

Stories are independent — US2 can start as soon as US1 T002 is done (or immediately, since T008 has no US1 dependency).

## Edge Cases (from spec)

- **Cascading script timeout**: Each script at each depth gets fresh `Instant::now()` before execution. A timeout at depth 2 terminates that script and its pending cascade writes, but writes committed at depths 0-1 are preserved (already the behavior — cascade writes are processed after each script returns).
- **Alternating error messages for rate limiter**: The rate limiter keys on message content hash. Two different messages alternate? Each has its own window — both emit normally. This is correct behavior.

## Risks

1. **`Instant` availability on ESP32**: `std::time::Instant` should work on ESP-IDF (backed by `esp_timer_get_time`). If not available, fall back to `esp_idf_svc::systime::EspSystemTime`.
2. **Post-execution timeout can't interrupt FFI calls**: If a script calls an FFI function that blocks for 30 seconds, the timeout only fires after `mjs_exec` returns. This is acknowledged — the op-count limit handles pure JS loops, and the post-execution check catches slow-but-returning FFI calls. A truly blocking FFI call would need a separate watchdog thread (out of scope for this spec).

# Implementation Plan: System Stats InfoItems

**Spec**: (inline — no separate spec.md)
**Branch**: `008-system-stats`
**Created**: 2026-03-08

## Summary

Add build-flag-gated system statistics as read-only InfoItems under `/System/`:
- **Temperature**: Internal chip temperature sensor (board-config gated, not all chips have it)
- **Memory stats**: Free flash (data partition), free NVS (`FreeOdfStorage`), free PSRAM (if `psram` feature)
- **Total metadata**: Each memory InfoItem gets `meta.total` so clients can compute percentages
- **FreeHeap enhancement**: Add `total` metadata to existing FreeHeap item

### O-DF tree layout

```
/System/FreeHeap        (existing, 5s cadence, add meta.total)
/System/Temperature     (°C, 5min cadence, board-config gated)
/System/FreeFlash       (bytes, data partition, 30s cadence)
/System/FreePsram       (bytes, psram feature only, 30s cadence)
/System/FreeOdfStorage  (bytes, NVS free space, 30s cadence)
```

### Feature flags

- `mem-stats`: Gates FreeFlash, FreePsram, FreeOdfStorage items (no new deps)
- Temperature: No feature flag — gated by board TOML `has_temp_sensor = true`

### Update cadences

- FreeHeap: 5s (unchanged)
- Temperature: 5min (300s)
- Memory stats (FreeFlash, FreePsram, FreeOdfStorage): 30s

---

## Tasks

### Task 1: Board config — temperature sensor field
**Priority**: P1 | **Dependencies**: none

Add `has_temp_sensor` boolean field to `[board]` section in board TOML schema.
Extend `build.rs` codegen to emit `pub const HAS_TEMP_SENSOR: bool` in
`gpio_config.rs`. Expose via `board::has_temp_sensor()` in `src/board.rs`
(returns false when no board config loaded).

Update `boards/esp32-s2-wrover.toml` with appropriate value for that chip.

**Files**: `build.rs`, `src/board.rs`, `boards/esp32-s2-wrover.toml`

**Acceptance**: `board::has_temp_sensor()` returns the TOML value when board config
loaded, false otherwise. Host tests pass.

---

### Task 2: Feature flag `mem-stats` and sensor tree scaffolding
**Priority**: P1 | **Dependencies**: Task 1

Add `mem-stats` feature to `Cargo.toml` (no external deps). Extend
`build_sensor_tree()` in `src/device.rs` to create new InfoItems:

- `FreeFlash` (always under `mem-stats`)
- `FreeOdfStorage` (always under `mem-stats`)
- `FreePsram` (under `mem-stats` AND `psram`)
- `Temperature` (under `has_temp_sensor` board config, no feature flag)

Each memory item gets `meta: { unit: "B", total: 0 }` (total set to 0 as
placeholder — populated at runtime on ESP after querying hardware).
Add `meta.total` to existing `FreeHeap` item.
Temperature gets `meta: { unit: "°C" }`.

Add path constants: `PATH_FREE_FLASH`, `PATH_FREE_PSRAM`,
`PATH_FREE_ODF_STORAGE`, `PATH_TEMPERATURE`.

**Files**: `Cargo.toml`, `src/device.rs`

**Acceptance**: `cargo check --features std,json,mem-stats` compiles.
`build_sensor_tree()` returns items with correct metadata. Host unit tests
verify item presence and metadata.

---

### Task 3: ESP temperature sensor driver
**Priority**: P2 | **Dependencies**: Task 1

Implement temperature reading behind `#[cfg(feature = "esp")]` +
`board::has_temp_sensor()` runtime check. Use `esp-idf-sys` raw FFI
(`temperature_sensor_install`, `temperature_sensor_enable`,
`temperature_sensor_get_celsius`) for broad chip compatibility.

Create `src/temp_sensor.rs` with:
- `TempSensor::new() -> Option<Self>` (returns None if not available)
- `TempSensor::read_celsius() -> Option<f64>`

**Files**: `src/temp_sensor.rs`, `src/lib.rs`

**Acceptance**: Compiles on ESP targets. Returns None gracefully on chips
without temperature sensor.

---

### Task 4: ESP memory stat readers
**Priority**: P1 | **Dependencies**: none

Implement memory query functions behind `#[cfg(feature = "esp")]` in
`src/mem_stats.rs`:

- `free_heap() -> u32` and `total_heap() -> u32` (existing `esp_get_free_heap_size` + `esp_get_heap_size` from esp-idf-sys, not custom)
- `free_flash() -> Option<(u32, u32)>` — (free, total) bytes of data partition
  via `esp_vfs_fat_info` or `esp_spiffs_info`, or partition table query
- `free_nvs() -> Option<(u32, u32)>` — (free, total) NVS stats via `nvs_get_stats`
- `free_psram() -> Option<(u32, u32)>` — (free, total) PSRAM via
  `heap_caps_get_free_size(MALLOC_CAP_SPIRAM)` / `heap_caps_get_total_size`

Provide host stubs returning None for testability.

**Files**: `src/mem_stats.rs`, `src/lib.rs`

**Acceptance**: Compiles on both host and ESP. ESP returns real values.
Host stubs return None.

---

### Task 5: Main loop integration
**Priority**: P1 | **Dependencies**: Tasks 2, 3, 4

Wire new items into `src/main.rs` main loop:

- At boot: query total values and set `meta.total` on each memory InfoItem
  (FreeHeap, FreeFlash, FreePsram, FreeOdfStorage)
- Add `MEMORY_INTERVAL_MS = 30_000` timer for FreeFlash, FreePsram, FreeOdfStorage
- Add `TEMP_INTERVAL_MS = 300_000` (5min) timer for Temperature
- Initialize TempSensor if `board::has_temp_sensor()`
- FreeHeap stays at existing 5s TICK_INTERVAL_MS

**Files**: `src/main.rs`

**Acceptance**: New items update at correct cadences. Temperature only sampled
when board supports it. No regressions in existing tick behavior.

---

### Task 6: Host tests
**Priority**: P2 | **Dependencies**: Tasks 2, 4

Unit tests in `src/device.rs` and `src/mem_stats.rs`:

- Sensor tree items present with correct type_uri, metadata, not writable
- Path constants resolve correctly
- `mem-stats` feature gates the right items
- `meta.total` present on all memory items
- Temperature item present only when `has_temp_sensor` board config set

**Files**: `src/device.rs`, `src/mem_stats.rs`

**Acceptance**: `cargo test-host --features mem-stats` passes.

---

### Task 7: E2E device validation
**Priority**: P3 | **Dependencies**: Tasks 5, 6

Flash device with `mem-stats` enabled. Read `/System/FreeFlash`,
`/System/FreeOdfStorage`, `/System/Temperature` (if applicable) via OMI read.
Verify values are plausible numbers with correct metadata.

**Files**: none (manual or e2e script extension)

**Acceptance**: All system stats items readable, values > 0, metadata.total > 0.

---

## Dependency Graph

```
Task 1 (board config)
  ├─► Task 2 (feature flag + tree scaffolding)
  │     └─► Task 5 (main loop) ◄── also 3, 4
  └─► Task 3 (temp sensor driver)
        └─► Task 5

Task 4 (mem stat readers) ──► Task 5 (main loop)

Tasks 2, 4 ──► Task 6 (host tests)

Tasks 5, 6 ──► Task 7 (e2e)
```

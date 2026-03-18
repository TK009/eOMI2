// Memory budget test for ESP32-S2.
//
// Parses the pre-built ELF binary to verify that DRAM, IRAM, and flash usage
// stay within the chip and module hardware limits.  Run after an ESP build:
//
//   rustup override set esp && cargo build
//   cargo test-host --test memory_budget
//
// Only compiles when the `esp` feature is disabled (host tests).

#![cfg(not(feature = "esp"))]

use object::read::elf::ElfFile32;
use object::{Endianness, Object, ObjectSection, SectionFlags};
use std::path::PathBuf;

// ── ESP32-S2 chip limits ────────────────────────────────────────────────────
// Source: ESP32-S2 Technical Reference Manual & datasheet.
//
// SRAM is shared between IRAM and DRAM; these are the maximum region sizes
// defined by the linker script.  The linker will reject a build that overflows
// a region, but this test catches regressions with a clear report.

/// Maximum DRAM (data + bss).  The linker region `dram0_0_seg` spans
/// 0x3FFB_0000 – 0x3FFF_FFFF (320 KB), but ~120 KB is reserved for system
/// use (Wi-Fi buffers, IDF heap, ROM data).  200 KB is a safe budget.
const DRAM_LIMIT: u64 = 200 * 1024;

/// Maximum IRAM (latency-critical code).  `iram0_0_seg` spans
/// 0x4002_0000 – 0x4003_FFFF (128 KB).
const IRAM_LIMIT: u64 = 128 * 1024;

/// Maximum flash image (code + read-only data stored on the SPI flash).
/// The smallest common ESP32-S2 module (ESP32-S2-MINI-1) has 4 MB flash.
/// With the OTA two-slot partition table, each app slot is 0x1E0000
/// (1,966,080 bytes = 1920 KB).  Storage partition reduced to 128 KB.
const FLASH_LIMIT: u64 = 0x1E_0000;

// ── ESP32-S2 address ranges ─────────────────────────────────────────────────

fn memory_region(addr: u64) -> &'static str {
    match addr {
        0x3FFB_0000..=0x3FFF_FFFF => "dram",
        0x4002_0000..=0x4003_FFFF => "iram",
        0x3F00_0000..=0x3F3F_FFFF => "flash", // data cache window
        0x4008_0000..=0x407F_FFFF => "flash", // instruction cache window
        _ => "other",
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn find_elf() -> Option<PathBuf> {
    // Prefer release (closer to production) then debug.
    for profile in ["release", "debug"] {
        let p = PathBuf::from(format!(
            "target/xtensa-esp32s2-espidf/{profile}/reconfigurable-device"
        ));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn is_alloc<'a>(section: &impl ObjectSection<'a>) -> bool {
    const SHF_ALLOC: u64 = 0x2;
    match section.flags() {
        SectionFlags::Elf { sh_flags } => sh_flags & SHF_ALLOC != 0,
        _ => false,
    }
}

// ── Test ────────────────────────────────────────────────────────────────────

#[test]
fn esp32s2_memory_fits() {
    let elf_path = match find_elf() {
        Some(p) => p,
        None => {
            eprintln!(
                "SKIP: ESP32-S2 ELF not found.  Build first:\n\
                 \n  rustup override set esp && cargo build\n"
            );
            return;
        }
    };

    let data = std::fs::read(&elf_path).expect("failed to read ELF binary");

    // ESP32-S2 (Xtensa LX7) is 32-bit little-endian.
    let elf = ElfFile32::<Endianness>::parse(&*data).expect("failed to parse ELF");

    let mut dram_bytes: u64 = 0;
    let mut iram_bytes: u64 = 0;
    let mut flash_bytes: u64 = 0;

    for section in elf.sections() {
        if !is_alloc(&section) {
            continue;
        }

        let addr = section.address();
        let memsz = section.size();

        match memory_region(addr) {
            "dram" => dram_bytes += memsz,
            "iram" => iram_bytes += memsz,
            "flash" => flash_bytes += memsz,
            _ => {}
        }
    }

    // DRAM .data initial values are also stored in flash.
    let flash_total = flash_bytes + dram_bytes.saturating_sub(bss_size(&elf));

    // ── Report ──────────────────────────────────────────────────────────
    let pct = |used: u64, limit: u64| used as f64 / limit as f64 * 100.0;

    eprintln!();
    eprintln!("  ESP32-S2 memory budget  ({})", elf_path.display());
    eprintln!("  ─────────────────────────────────────────────────");
    eprintln!(
        "  DRAM   {:>7.1} KB / {:>5.0} KB  ({:5.1}%)",
        dram_bytes as f64 / 1024.0,
        DRAM_LIMIT as f64 / 1024.0,
        pct(dram_bytes, DRAM_LIMIT),
    );
    eprintln!(
        "  IRAM   {:>7.1} KB / {:>5.0} KB  ({:5.1}%)",
        iram_bytes as f64 / 1024.0,
        IRAM_LIMIT as f64 / 1024.0,
        pct(iram_bytes, IRAM_LIMIT),
    );
    eprintln!(
        "  Flash  {:>7.1} KB / {:>5.0} KB  ({:5.1}%)",
        flash_total as f64 / 1024.0,
        FLASH_LIMIT as f64 / 1024.0,
        pct(flash_total, FLASH_LIMIT),
    );
    eprintln!();

    // ── Assertions ──────────────────────────────────────────────────────
    assert!(
        dram_bytes <= DRAM_LIMIT,
        "DRAM overflow: {} bytes used, limit is {} bytes ({} KB over)",
        dram_bytes,
        DRAM_LIMIT,
        (dram_bytes - DRAM_LIMIT) / 1024,
    );
    assert!(
        iram_bytes <= IRAM_LIMIT,
        "IRAM overflow: {} bytes used, limit is {} bytes ({} KB over)",
        iram_bytes,
        IRAM_LIMIT,
        (iram_bytes - IRAM_LIMIT) / 1024,
    );
    assert!(
        flash_total <= FLASH_LIMIT,
        "Flash overflow: {} bytes used, limit is {} bytes ({} KB over)",
        flash_total,
        FLASH_LIMIT,
        (flash_total - FLASH_LIMIT) / 1024,
    );
}

/// Sum of BSS (zero-initialised) sections in the DRAM region.
/// BSS occupies DRAM at runtime but has no flash footprint.
fn bss_size(elf: &ElfFile32<Endianness>) -> u64 {
    let mut total = 0u64;
    for section in elf.sections() {
        if !is_alloc(&section) {
            continue;
        }
        let addr = section.address();
        if memory_region(addr) != "dram" {
            continue;
        }
        // BSS sections have memory size > 0 but no file backing.
        if section.file_range().is_none() && section.size() > 0 {
            total += section.size();
        }
    }
    total
}

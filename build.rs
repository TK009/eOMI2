// --- Board TOML config parsing (FR-001, FR-011, FR-012, FR-013) ---

#[cfg(feature = "gpio")]
mod board_config {
    use serde::Deserialize;
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    const VALID_MODES: &[&str] = &[
        "digital_in",
        "digital_out",
        "analog_in",
        "pwm",
        "low_edge_trigger",
        "high_edge_trigger",
    ];

    const VALID_PROTOCOLS: &[&str] = &["I2C", "SPI", "UART"];

    #[derive(Deserialize)]
    pub struct BoardFile {
        pub board: BoardMeta,
        #[serde(default)]
        pub gpio: Vec<GpioEntry>,
        #[serde(default)]
        pub peripheral: Vec<PeripheralEntry>,
    }

    #[derive(Deserialize)]
    pub struct BoardMeta {
        pub name: String,
        pub chip: String,
        #[serde(default)]
        pub has_temp_sensor: bool,
    }

    #[derive(Deserialize)]
    pub struct GpioEntry {
        pub pin: u8,
        pub mode: String,
        pub name: Option<String>,
    }

    #[derive(Deserialize)]
    pub struct PeripheralEntry {
        pub protocol: String,
        pub sda: Option<u8>,
        pub scl: Option<u8>,
        pub rx: Option<u8>,
        pub tx: Option<u8>,
        pub mosi: Option<u8>,
        pub miso: Option<u8>,
        pub sck: Option<u8>,
        pub cs: Option<u8>,
    }

    impl PeripheralEntry {
        /// Collect all pin assignments as (pin, role) pairs.
        fn pins(&self) -> Vec<(u8, &'static str)> {
            let mut out = Vec::new();
            if let Some(p) = self.sda { out.push((p, "sda")); }
            if let Some(p) = self.scl { out.push((p, "scl")); }
            if let Some(p) = self.rx { out.push((p, "rx")); }
            if let Some(p) = self.tx { out.push((p, "tx")); }
            if let Some(p) = self.mosi { out.push((p, "mosi")); }
            if let Some(p) = self.miso { out.push((p, "miso")); }
            if let Some(p) = self.sck { out.push((p, "sck")); }
            if let Some(p) = self.cs { out.push((p, "cs")); }
            out
        }
    }

    /// Parse a board TOML file and return the deserialized config.
    pub fn parse(path: &Path) -> BoardFile {
        let contents = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read board config {}: {}", path.display(), e));
        toml::from_str(&contents)
            .unwrap_or_else(|e| panic!("failed to parse board config {}: {}", path.display(), e))
    }

    /// Validate modes, protocols, and detect pin conflicts (FR-011).
    /// Panics with a descriptive message on any conflict.
    pub fn validate(board: &BoardFile) {
        // Validate GPIO modes
        for entry in &board.gpio {
            if !VALID_MODES.contains(&entry.mode.as_str()) {
                panic!(
                    "board config: GPIO pin {} has invalid mode '{}'. Valid modes: {:?}",
                    entry.pin, entry.mode, VALID_MODES
                );
            }
        }

        // Validate peripheral protocols
        for entry in &board.peripheral {
            if !VALID_PROTOCOLS.contains(&entry.protocol.as_str()) {
                panic!(
                    "board config: peripheral has invalid protocol '{}'. Valid: {:?}",
                    entry.protocol, VALID_PROTOCOLS
                );
            }
        }

        // FR-011: Detect conflicting GPIO pin assignments
        let mut pin_owners: HashMap<u8, String> = HashMap::new();

        for entry in &board.gpio {
            let label = entry
                .name
                .as_deref()
                .map(|n| format!("gpio '{}' (pin {})", n, entry.pin))
                .unwrap_or_else(|| format!("gpio pin {}", entry.pin));
            if let Some(prev) = pin_owners.insert(entry.pin, label.clone()) {
                panic!(
                    "board config: pin conflict on GPIO {}: used by both {} and {}",
                    entry.pin, prev, label
                );
            }
        }

        for entry in &board.peripheral {
            for (pin, role) in entry.pins() {
                let label = format!("{} {} (pin {})", entry.protocol, role, pin);
                if let Some(prev) = pin_owners.insert(pin, label.clone()) {
                    panic!(
                        "board config: pin conflict on GPIO {}: used by both {} and {}",
                        pin, prev, label
                    );
                }
            }
        }

        // Validate unique InfoItem names
        let mut names = HashSet::new();
        for entry in &board.gpio {
            let name = entry
                .name
                .clone()
                .unwrap_or_else(|| format!("GPIO{}", entry.pin));
            if !names.insert(name.clone()) {
                panic!(
                    "board config: duplicate InfoItem name '{}' (pin {})",
                    name, entry.pin
                );
            }
        }
    }

    /// Generate gpio_config.rs const arrays into OUT_DIR.
    pub fn generate(board: &BoardFile, out_dir: &Path) {
        let mut code = String::new();

        code.push_str("// Auto-generated by build.rs from board TOML config.\n");
        code.push_str("// Do not edit manually.\n\n");

        // Board metadata
        code.push_str(&format!(
            "pub const BOARD_NAME: &str = {:?};\n",
            board.board.name
        ));
        code.push_str(&format!(
            "pub const BOARD_CHIP: &str = {:?};\n",
            board.board.chip
        ));
        code.push_str(&format!(
            "pub const HAS_TEMP_SENSOR: bool = {};\n\n",
            board.board.has_temp_sensor
        ));

        // GPIO config: &[(pin, mode, name)]
        code.push_str("/// Build-time GPIO pin configurations.\n");
        code.push_str("/// Each entry: (pin_number, mode_str, infoitem_name).\n");
        code.push_str(
            "pub const GPIO_CONFIGS: &[(u8, &str, &str)] = &[\n",
        );
        for entry in &board.gpio {
            let name = entry
                .name
                .clone()
                .unwrap_or_else(|| format!("GPIO{}", entry.pin));
            code.push_str(&format!(
                "    ({}, {:?}, {:?}),\n",
                entry.pin, entry.mode, name
            ));
        }
        code.push_str("];\n\n");

        // Peripheral config: &[(protocol, &[(pin, role)])]
        // Since nested slices aren't trivial in const, generate per-peripheral
        // pin arrays and a summary array.
        code.push_str("/// Build-time peripheral protocol configurations.\n");
        code.push_str(
            "/// Each entry: (protocol, pin_pairs) where pin_pairs is &[(pin, role)].\n",
        );

        for (i, entry) in board.peripheral.iter().enumerate() {
            let pins = entry.pins();
            code.push_str(&format!(
                "const PERIPH_{}_PINS: &[(u8, &str)] = &[",
                i
            ));
            for (pin, role) in &pins {
                code.push_str(&format!("({}, {:?}), ", pin, role));
            }
            code.push_str("];\n");
        }

        code.push_str(
            "\npub const PERIPHERAL_CONFIGS: &[(&str, &[(u8, &str)])] = &[\n",
        );
        for (i, entry) in board.peripheral.iter().enumerate() {
            code.push_str(&format!(
                "    ({:?}, PERIPH_{}_PINS),\n",
                entry.protocol, i
            ));
        }
        code.push_str("];\n");

        let out_file = out_dir.join("gpio_config.rs");
        std::fs::write(&out_file, code).unwrap_or_else(|e| {
            panic!("failed to write {}: {}", out_file.display(), e)
        });
    }
}

fn main() {
    // Declare the has_board_config cfg for check-cfg validation
    println!("cargo::rustc-check-cfg=cfg(has_board_config)");

    #[cfg(feature = "scripting")]
    {
        println!("cargo:rerun-if-changed=vendor/mjs/mjs.c");
        println!("cargo:rerun-if-changed=vendor/mjs/mjs.h");
        let mut build = cc::Build::new();
        build
            .file("vendor/mjs/mjs.c")
            .include("vendor/mjs")
            .define("CS_ENABLE_STDIO", Some("0"))
            .opt_level_str("s")
            .warnings(false);
        // When cross-compiling for ESP-IDF targets, the xtensa toolchain
        // doesn't define ESP_PLATFORM automatically. mJS needs it to select
        // the correct platform headers.
        let target = std::env::var("TARGET").unwrap_or_default();
        if target.contains("espidf") {
            build.define("ESP_PLATFORM", None);
        }
        // The cc crate can't auto-detect the xtensa cross-compiler from the
        // ESP-IDF target triple. Point it at the correct GCC explicitly.
        if target.contains("xtensa") && target.contains("espidf") {
            // e.g. "xtensa-esp32s2-espidf" → "xtensa-esp32s2-elf-gcc"
            let gcc = target.replace("-espidf", "-elf-gcc");
            build.compiler(&gcc);
            // Xtensa call8 instructions have limited range; -mlongcalls
            // generates indirect calls so large compilation units like
            // mjs.c don't produce "call target out of range" linker errors.
            build.flag("-mlongcalls");
        }
        build.compile("mjs");
    }

    #[cfg(all(feature = "esp", feature = "psram"))]
    {
        // SAFETY: build.rs is single-threaded; set_var is fine here.
        unsafe {
            std::env::set_var(
                "ESP_IDF_SDKCONFIG_DEFAULTS",
                "sdkconfig.defaults;sdkconfig.psram.defaults",
            );
        }
    }

    #[cfg(feature = "esp")]
    {
        embuild::espidf::sysenv::output();

        const ALLOWED_ENV_KEYS: &[&str] = &["WIFI_SSID", "WIFI_PASS", "API_TOKEN"];

        // Always track .env so Cargo re-runs build.rs when it appears or changes
        println!("cargo:rerun-if-changed=.env");

        // Collect env vars from .env file (if it exists)
        let mut env_map = std::collections::HashMap::new();
        if let Ok(contents) = std::fs::read_to_string(".env") {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    if ALLOWED_ENV_KEYS.contains(&key) {
                        let value = value.trim();
                        // Strip surrounding quotes if present
                        let value = value
                            .strip_prefix('"')
                            .and_then(|v| v.strip_suffix('"'))
                            .or_else(|| {
                                value.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))
                            })
                            .unwrap_or(value);
                        env_map.insert(key.to_string(), value.to_string());
                    }
                }
            }
        }

        // Pass whitelisted keys as compile-time env vars (now optional)
        for (key, value) in &env_map {
            println!("cargo:rustc-env={}={}", key, value);
        }

    }

    // --- Build-configurable constants (available in all build profiles) ---
    println!("cargo:rerun-if-env-changed=MAX_WIFI_APS");
    println!("cargo:rerun-if-env-changed=EOMI_HOSTNAME");
    println!("cargo:rerun-if-env-changed=PERIPHERALS");

    // MAX_WIFI_APS: default 3
    let max_wifi_aps: usize = std::env::var("MAX_WIFI_APS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(
        std::path::Path::new(&out_dir).join("max_wifi_aps.const"),
        format!("{}", max_wifi_aps),
    )
    .expect("Failed to write max_wifi_aps.const");

    // PERIPHERALS: comma-separated list of protocol:name pairs (e.g. "I2C:GPIO21,UART:GPIO16,SPI:GPIO18")
    // Parsed at runtime by PeripheralConfig. Empty string means no peripherals configured.
    let peripherals = std::env::var("PERIPHERALS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    println!("cargo:rustc-env=PERIPHERALS={}", peripherals);

    // HOSTNAME: default "eOMI"
    let hostname = std::env::var("EOMI_HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "eOMI".to_string());
    println!("cargo:rustc-env=EOMI_HOSTNAME={}", hostname);

    // --- Board TOML config → gpio_config.rs (FR-001, FR-011, FR-012, FR-013) ---
    #[cfg(feature = "gpio")]
    {
        println!("cargo:rerun-if-env-changed=EOMI_BOARD");

        if let Ok(board_name) = std::env::var("EOMI_BOARD") {
            if !board_name.is_empty() {
                let board_path =
                    std::path::Path::new("boards").join(format!("{}.toml", board_name));
                println!(
                    "cargo:rerun-if-changed={}",
                    board_path.display()
                );

                let board = board_config::parse(&board_path);
                board_config::validate(&board);

                let out_dir = std::path::Path::new(&out_dir);
                board_config::generate(&board, out_dir);

                // Tell downstream code that a board config was loaded
                println!("cargo:rustc-cfg=has_board_config");
            }
        }
    }
}

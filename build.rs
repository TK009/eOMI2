fn main() {
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
}

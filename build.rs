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

        // Load .env and pass whitelisted keys as compile-time env vars
        let contents = std::fs::read_to_string(".env")
            .expect("Missing .env file. Copy .env.example to .env and set WIFI_SSID and WIFI_PASS");

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
                        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                        .unwrap_or(value);
                    println!("cargo:rustc-env={}={}", key, value);
                }
            }
        }
    }
}

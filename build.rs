fn main() {
    #[cfg(feature = "esp")]
    {
        embuild::espidf::sysenv::output();

        const ALLOWED_ENV_KEYS: &[&str] = &["WIFI_SSID", "WIFI_PASS"];

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

fn main() {
    embuild::espidf::sysenv::output();

    // Load .env and pass WIFI_SSID / WIFI_PASS as compile-time env vars
    if let Ok(contents) = std::fs::read_to_string(".env") {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                println!("cargo:rustc-env={}={}", key.trim(), value.trim());
            }
        }
        println!("cargo:rerun-if-changed=.env");
    }
}

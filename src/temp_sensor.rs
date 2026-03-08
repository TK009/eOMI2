//! ESP internal temperature sensor driver (spec 008-T03).
//!
//! Wraps the esp-idf `temperature_sensor` API behind a runtime
//! [`board::has_temp_sensor()`] check so chips without the peripheral
//! return `None` gracefully instead of panicking.

use log::{info, warn};

/// Handle to the on-chip temperature sensor.
pub struct TempSensor {
    handle: esp_idf_svc::sys::temperature_sensor_handle_t,
}

impl TempSensor {
    /// Install and enable the temperature sensor.
    ///
    /// Returns `None` if the board config says this chip has no sensor,
    /// or if driver installation / enable fails at runtime.
    pub fn new() -> Option<Self> {
        if !crate::board::has_temp_sensor() {
            info!("TempSensor: board config says no temp sensor — skipping");
            return None;
        }

        unsafe {
            let cfg = esp_idf_svc::sys::temperature_sensor_config_t {
                range_min: -10,
                range_max: 80,
                ..Default::default()
            };

            let mut handle: esp_idf_svc::sys::temperature_sensor_handle_t =
                core::ptr::null_mut();

            let ret = esp_idf_svc::sys::temperature_sensor_install(&cfg, &mut handle);
            if ret != esp_idf_svc::sys::ESP_OK {
                warn!(
                    "TempSensor: temperature_sensor_install failed (err={})",
                    ret
                );
                return None;
            }

            let ret = esp_idf_svc::sys::temperature_sensor_enable(handle);
            if ret != esp_idf_svc::sys::ESP_OK {
                warn!(
                    "TempSensor: temperature_sensor_enable failed (err={})",
                    ret
                );
                // Clean up the installed-but-not-enabled handle.
                esp_idf_svc::sys::temperature_sensor_uninstall(handle);
                return None;
            }

            info!("TempSensor: initialised (range -10 .. 80 °C)");
            Some(Self { handle })
        }
    }

    /// Read the current die temperature in degrees Celsius.
    ///
    /// Returns `None` if the underlying driver call fails.
    pub fn read_celsius(&self) -> Option<f64> {
        let mut celsius: f32 = 0.0;
        let ret = unsafe {
            esp_idf_svc::sys::temperature_sensor_get_celsius(self.handle, &mut celsius)
        };
        if ret != esp_idf_svc::sys::ESP_OK {
            warn!("TempSensor: read failed (err={})", ret);
            return None;
        }
        Some(celsius as f64)
    }
}

impl Drop for TempSensor {
    fn drop(&mut self) {
        unsafe {
            esp_idf_svc::sys::temperature_sensor_disable(self.handle);
            esp_idf_svc::sys::temperature_sensor_uninstall(self.handle);
        }
        info!("TempSensor: disabled and uninstalled");
    }
}

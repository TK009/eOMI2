// DHT11 temperature & humidity sensor driver for ESP32.
//
// Uses GPIO open-drain mode with bit-banged timing.
// The DHT11 protocol: host pulls low 20 ms to request data,
// sensor responds with 40 bits (16 humidity + 16 temperature + 8 checksum).

use esp_idf_svc::hal::delay::Ets;
use esp_idf_svc::hal::gpio::{InputOutput, OpenDrain, PinDriver};
use esp_idf_svc::hal::gpio::AnyIOPin;

/// A successful DHT11 reading.
#[derive(Debug, Clone)]
pub struct Dht11Reading {
    pub temperature: f32,
    pub humidity: f32,
}

/// Errors from reading the DHT11 sensor.
#[derive(Debug)]
pub enum Dht11Error {
    /// Sensor did not respond to the start signal.
    NoResponse,
    /// Timed out waiting for a bit transition.
    Timeout,
    /// Checksum mismatch in received data.
    Checksum,
}

impl core::fmt::Display for Dht11Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Dht11Error::NoResponse => write!(f, "DHT11: no response"),
            Dht11Error::Timeout => write!(f, "DHT11: timeout"),
            Dht11Error::Checksum => write!(f, "DHT11: checksum mismatch"),
        }
    }
}

/// Maximum microseconds to spin-wait for a pin level change.
const TIMEOUT_US: u32 = 1000;

/// Wait for the pin to reach `level`, returning elapsed microseconds.
/// Returns `Err(Timeout)` if `TIMEOUT_US` is exceeded.
fn wait_for_level(
    pin: &PinDriver<'_, AnyIOPin, InputOutput<OpenDrain>>,
    level: bool,
) -> Result<u32, Dht11Error> {
    let mut elapsed: u32 = 0;
    while pin.is_high() != level {
        Ets::delay_us(1);
        elapsed += 1;
        if elapsed > TIMEOUT_US {
            return Err(Dht11Error::Timeout);
        }
    }
    Ok(elapsed)
}

/// Read temperature and humidity from a DHT11 sensor.
///
/// The pin must be configured as `InputOutput<OpenDrain>` (open-drain mode).
///
/// # Preconditions
///
/// The caller **must** wait at least **1 second** between successive calls.
/// The DHT11 hardware requires this minimum sampling interval; calling more
/// frequently will produce timeouts or corrupt readings. There is no runtime
/// guard — the caller is responsible for enforcing the interval (e.g. via a
/// `sleep` or timer in the main loop).
pub fn read_dht11(
    pin: &mut PinDriver<'_, AnyIOPin, InputOutput<OpenDrain>>,
) -> Result<Dht11Reading, Dht11Error> {
    // --- Start signal: pull low for 20 ms, then release ---
    pin.set_low().map_err(|_| Dht11Error::NoResponse)?;
    Ets::delay_us(20_000);
    pin.set_high().map_err(|_| Dht11Error::NoResponse)?;

    // --- Wait for sensor response (DHT11 datasheet timing) ---
    // After host releases the bus, the line floats high briefly.
    // Step 1: 40 μs delay — skip past the host release period while
    //         the bus is still high (pull-up) before the sensor responds.
    Ets::delay_us(40);
    // Step 2: Sensor pulls low for ~80 μs as its "response" signal.
    //         Wait until the line goes low (sensor has started responding).
    wait_for_level(pin, false)?;
    // Step 3: Sensor releases the line — it goes high for ~80 μs as the
    //         "preparation" period before data transmission begins.
    wait_for_level(pin, true)?;
    // Step 4: Sensor pulls low again to start the first data bit's ~50 μs
    //         low preamble. After this, we enter the 40-bit data loop.
    wait_for_level(pin, false)?;

    // --- Read 40 data bits ---
    let mut data = [0u8; 5];
    for byte in 0..5 {
        for bit in (0..8).rev() {
            // Each bit starts with ~50 us low
            wait_for_level(pin, true)?;
            // Then high: ~26-28 us = 0, ~70 us = 1
            let high_us = wait_for_level(pin, false)?;
            if high_us > 40 {
                data[byte] |= 1 << bit;
            }
        }
    }

    // --- Validate checksum ---
    let checksum = data[0]
        .wrapping_add(data[1])
        .wrapping_add(data[2])
        .wrapping_add(data[3]);
    if checksum != data[4] {
        return Err(Dht11Error::Checksum);
    }

    // DHT11: data[0] = humidity integer, data[1] = humidity decimal (usually 0)
    //        data[2] = temperature integer, data[3] = temperature decimal
    let humidity = data[0] as f32 + data[1] as f32 * 0.1;
    let temperature = data[2] as f32 + (data[3] & 0x7F) as f32 * 0.1;
    // Bit 7 of data[3] indicates negative temperature
    let temperature = if data[3] & 0x80 != 0 {
        -temperature
    } else {
        temperature
    };

    Ok(Dht11Reading {
        temperature,
        humidity,
    })
}

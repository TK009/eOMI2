// WSOP cryptographic primitives — host-testable.
//
// X25519 key exchange, BLAKE2b verification code derivation,
// and NaCl crypto_box_seal encryption/decryption.

extern crate alloc;

use alloc::vec::Vec;
use blake2::{digest::consts::U1, Blake2b, Digest};
use crypto_box::{
    aead::OsRng,
    PublicKey, SecretKey,
};

/// X25519 keypair for onboarding.
pub struct Keypair {
    pub secret: SecretKey,
    pub public: PublicKey,
}

impl Keypair {
    /// Generate a new random X25519 keypair.
    pub fn generate() -> Self {
        let secret = SecretKey::generate(&mut OsRng);
        let public = secret.public_key().clone();
        Self { secret, public }
    }

    /// Reconstruct from raw 32-byte secret key.
    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        let secret = SecretKey::from(bytes);
        let public = secret.public_key().clone();
        Self { secret, public }
    }

    /// Raw 32-byte public key.
    pub fn public_bytes(&self) -> [u8; 32] {
        *self.public.as_bytes()
    }
}

/// RGB color for LED verification display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Verification code derived from a public key via BLAKE2b(pubkey, 1 byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyCode {
    pub byte: u8,
}

/// Color table: top 3 bits of verify byte → RGB.
const COLORS: [(u8, u8, u8); 8] = [
    (255, 0, 0),     // 000 Red
    (0, 255, 0),     // 001 Green
    (0, 0, 255),     // 010 Blue
    (255, 255, 0),   // 011 Yellow
    (0, 255, 255),   // 100 Cyan
    (255, 0, 255),   // 101 Magenta
    (255, 255, 255), // 110 White
    (255, 128, 0),   // 111 Orange
];

impl VerifyCode {
    /// Derive verification code from a 32-byte X25519 public key.
    ///
    /// Uses unkeyed BLAKE2b with 1-byte output.
    pub fn from_pubkey(pubkey: &[u8; 32]) -> Self {
        let mut hasher = Blake2b::<U1>::new();
        hasher.update(pubkey);
        let result = hasher.finalize();
        Self { byte: result[0] }
    }

    /// Color index (top 3 bits): 0..7 mapping to 8 colors.
    pub fn color_index(&self) -> u8 {
        self.byte >> 5
    }

    /// RGB color for LED display.
    pub fn color(&self) -> VerifyColor {
        let (r, g, b) = COLORS[self.color_index() as usize];
        VerifyColor { r, g, b }
    }

    /// Digit for numeric display: byte mod 10 → 0..9.
    pub fn digit(&self) -> u8 {
        self.byte % 10
    }
}

/// Encrypt plaintext using NaCl sealed box (crypto_box_seal).
///
/// The ciphertext includes a 32-byte ephemeral public key and a 16-byte
/// Poly1305 MAC (48 bytes overhead total).
pub fn seal(plaintext: &[u8], recipient_pubkey: &PublicKey) -> Vec<u8> {
    recipient_pubkey
        .seal(&mut OsRng, plaintext)
        .expect("seal encryption should not fail")
}

/// Decrypt sealed-box ciphertext (crypto_box_seal_open).
///
/// Returns `None` if decryption fails (wrong key or tampered data).
pub fn seal_open(ciphertext: &[u8], secret_key: &SecretKey) -> Option<Vec<u8>> {
    secret_key.unseal(ciphertext).ok()
}

/// Sealed-box overhead: 32-byte ephemeral pubkey + 16-byte MAC.
pub const SEAL_OVERHEAD: usize = 48;

// On ESP-IDF targets, register the custom getrandom backend using
// esp_fill_random. This ensures x25519-dalek and crypto_box can
// access hardware RNG without std.
#[cfg(all(feature = "esp", target_os = "espidf"))]
mod esp_rng {
    use getrandom::register_custom_getrandom;

    fn esp_custom_getrandom(buf: &mut [u8]) -> Result<(), getrandom::Error> {
        // SAFETY: esp_fill_random fills the buffer with hardware RNG data.
        // Available on all ESP-IDF targets.
        unsafe {
            esp_idf_svc::sys::esp_fill_random(buf.as_mut_ptr() as *mut _, buf.len());
        }
        Ok(())
    }

    register_custom_getrandom!(esp_custom_getrandom);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keygen_produces_different_keys() {
        let kp1 = Keypair::generate();
        let kp2 = Keypair::generate();
        assert_ne!(kp1.public_bytes(), kp2.public_bytes());
    }

    #[test]
    fn from_secret_bytes_roundtrip() {
        let kp = Keypair::generate();
        let secret_bytes: [u8; 32] = kp.secret.to_bytes();
        let kp2 = Keypair::from_secret_bytes(secret_bytes);
        assert_eq!(kp.public_bytes(), kp2.public_bytes());
    }

    #[test]
    fn verify_code_deterministic() {
        let pubkey = [42u8; 32];
        let v1 = VerifyCode::from_pubkey(&pubkey);
        let v2 = VerifyCode::from_pubkey(&pubkey);
        assert_eq!(v1, v2);
    }

    #[test]
    fn verify_code_color_index_range() {
        // All possible byte values should map to color index 0..7
        for b in 0..=255u8 {
            let vc = VerifyCode { byte: b };
            assert!(vc.color_index() < 8);
        }
    }

    #[test]
    fn verify_code_digit_range() {
        for b in 0..=255u8 {
            let vc = VerifyCode { byte: b };
            assert!(vc.digit() < 10);
        }
    }

    #[test]
    fn verify_code_color_mapping() {
        // byte 0b000_xxxxx → color index 0 → Red
        let vc = VerifyCode { byte: 0b000_00000 };
        assert_eq!(vc.color(), VerifyColor { r: 255, g: 0, b: 0 });

        // byte 0b111_xxxxx → color index 7 → Orange
        let vc = VerifyCode { byte: 0b111_00000 };
        assert_eq!(vc.color(), VerifyColor { r: 255, g: 128, b: 0 });
    }

    #[test]
    fn seal_roundtrip() {
        let kp = Keypair::generate();
        let plaintext = b"hello WSOP";
        let ciphertext = seal(plaintext, &kp.public);
        assert_eq!(ciphertext.len(), plaintext.len() + SEAL_OVERHEAD);

        let decrypted = seal_open(&ciphertext, &kp.secret).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn seal_wrong_key_fails() {
        let kp1 = Keypair::generate();
        let kp2 = Keypair::generate();
        let ciphertext = seal(b"secret", &kp1.public);
        assert!(seal_open(&ciphertext, &kp2.secret).is_none());
    }

    #[test]
    fn seal_tampered_fails() {
        let kp = Keypair::generate();
        let mut ciphertext = seal(b"secret", &kp.public);
        // Flip a byte
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        assert!(seal_open(&ciphertext, &kp.secret).is_none());
    }
}

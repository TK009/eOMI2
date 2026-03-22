// WiFi Secure Onboarding Protocol (WSOP) — host-testable modules.
//
// Provides crypto primitives, wire-format serialization, a
// platform-independent joiner state machine, and verification display
// for secure device onboarding.

#[cfg(feature = "secure_onboarding")]
pub mod crypto;
#[cfg(feature = "esp")]
pub mod display;
#[cfg(all(feature = "esp", feature = "secure_onboarding"))]
pub mod joiner;
#[cfg(feature = "secure_onboarding")]
pub mod onboard_sm;
#[cfg(feature = "secure_onboarding")]
pub mod protocol;

#[cfg(all(test, feature = "secure_onboarding"))]
mod tests {
    use super::crypto::{self, Keypair, VerifyCode};
    use super::protocol::{
        JoinRequest, JoinResponse, WifiCredentials, SecurityType,
        STATUS_APPROVED, STATUS_DENIED,
    };

    /// Full round-trip: keygen → serialize request → seal credentials →
    /// deserialize response → open. Covers FR-100–FR-104, FR-112/FR-114.
    #[test]
    fn full_onboarding_roundtrip() {
        // 1. Device generates keypair
        let device_kp = Keypair::generate();
        let pubkey = device_kp.public_bytes();

        // 2. Both sides derive the same verification code
        let device_code = VerifyCode::from_pubkey(&pubkey);
        let gateway_code = VerifyCode::from_pubkey(&pubkey);
        assert_eq!(device_code, gateway_code);
        assert!(device_code.color_index() < 8);
        assert!(device_code.digit() < 10);

        // 3. Device serializes JOIN_REQUEST
        let nonce = [0x42u8; 8];
        let request = JoinRequest {
            name: String::from("my-sensor"),
            mac: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            pubkey,
            nonce,
            timestamp: 1700000000,
        };
        let request_bytes = request.serialize().unwrap();

        // 4. Gateway deserializes JOIN_REQUEST
        let parsed_req = JoinRequest::deserialize(&request_bytes).unwrap();
        assert_eq!(parsed_req, request);

        // 5. Gateway encrypts credentials with device's public key
        let creds = WifiCredentials {
            ssid: String::from("HomeNetwork"),
            security_type: SecurityType::Wpa2Psk,
            credential: String::from("supersecret"),
        };
        let creds_bytes = creds.serialize().unwrap();
        let sealed = crypto::seal(&creds_bytes, &device_kp.public);

        // 6. Gateway builds JOIN_RESPONSE
        let response = JoinResponse {
            nonce_echo: parsed_req.nonce,
            status: STATUS_APPROVED,
            ciphertext: sealed,
        };
        let response_bytes = response.serialize();

        // 7. Device deserializes JOIN_RESPONSE
        let parsed_resp = JoinResponse::deserialize(&response_bytes).unwrap();
        assert_eq!(parsed_resp.nonce_echo, nonce);
        assert_eq!(parsed_resp.status, STATUS_APPROVED);

        // 8. Device decrypts credentials
        let decrypted = crypto::seal_open(&parsed_resp.ciphertext, &device_kp.secret)
            .expect("device should be able to decrypt credentials");
        let got_creds = WifiCredentials::deserialize(&decrypted).unwrap();
        assert_eq!(got_creds, creds);
    }

    /// Verify that a wrong keypair cannot decrypt the response.
    #[test]
    fn wrong_key_cannot_decrypt_response() {
        let device_kp = Keypair::generate();
        let attacker_kp = Keypair::generate();

        let creds = WifiCredentials {
            ssid: String::from("HomeNetwork"),
            security_type: SecurityType::Wpa2Psk,
            credential: String::from("supersecret"),
        };
        let creds_bytes = creds.serialize().unwrap();
        let sealed = crypto::seal(&creds_bytes, &device_kp.public);

        // Attacker cannot decrypt
        assert!(crypto::seal_open(&sealed, &attacker_kp.secret).is_none());
    }

    /// Denied response has empty ciphertext and correct nonce echo.
    #[test]
    fn denied_response_roundtrip() {
        let nonce = [0x99u8; 8];
        let response = JoinResponse {
            nonce_echo: nonce,
            status: STATUS_DENIED,
            ciphertext: vec![],
        };
        let bytes = response.serialize();
        let parsed = JoinResponse::deserialize(&bytes).unwrap();
        assert_eq!(parsed.nonce_echo, nonce);
        assert_eq!(parsed.status, STATUS_DENIED);
        assert!(parsed.ciphertext.is_empty());
    }
}

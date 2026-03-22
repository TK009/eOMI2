// Full round-trip integration test for WSOP crypto + protocol.
//
// Tests the complete onboarding flow: keygen → serialize request →
// seal credentials → deserialize response → open. Verifies color
// derivation, nonce matching, and auth failure rejection.

#![cfg(feature = "secure_onboarding")]

use reconfigurable_device::wsop::{
    crypto::{self, Keypair, VerifyCode, SEAL_OVERHEAD},
    protocol::{
        JoinRequest, JoinResponse, SecurityType, WifiCredentials,
        STATUS_APPROVED, STATUS_DENIED, MAX_JOIN_REQUEST,
        base64_encode, base64_decode,
    },
};

/// Full happy-path: keygen → request → seal → response → open.
#[test]
fn full_roundtrip_onboarding() {
    // 1. Device generates keypair
    let device_kp = Keypair::generate();
    let pubkey = device_kp.public_bytes();

    // 2. Device computes and checks verify code
    let code = VerifyCode::from_pubkey(&pubkey);
    assert!(code.color_index() < 8);
    assert!(code.digit() < 10);

    // 3. Device serializes JOIN_REQUEST
    let nonce = [0xAA; 8];
    let request = JoinRequest {
        name: String::from("my-iot-device"),
        mac: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
        pubkey,
        nonce,
        timestamp: 1700000000,
    };
    let request_bytes = request.serialize().unwrap();
    assert!(request_bytes.len() <= MAX_JOIN_REQUEST);

    // 4. Gateway deserializes the request
    let parsed_request = JoinRequest::deserialize(&request_bytes).unwrap();
    assert_eq!(parsed_request.name, "my-iot-device");
    assert_eq!(parsed_request.pubkey, pubkey);
    assert_eq!(parsed_request.nonce, nonce);

    // 5. Gateway computes verify code from received pubkey (should match device's)
    let gw_code = VerifyCode::from_pubkey(&parsed_request.pubkey);
    assert_eq!(code, gw_code);

    // 6. Gateway prepares credentials and seals them to device's pubkey
    let credentials = WifiCredentials {
        ssid: String::from("HomeWiFi"),
        security_type: SecurityType::Wpa2Psk,
        credential: String::from("super-secret-password"),
    };
    let cred_bytes = credentials.serialize().unwrap();
    let recipient_pk = crypto_box::PublicKey::from(parsed_request.pubkey);
    let ciphertext = crypto::seal(&cred_bytes, &recipient_pk);
    assert_eq!(ciphertext.len(), cred_bytes.len() + SEAL_OVERHEAD);

    // 7. Gateway sends JOIN_RESPONSE
    let response = JoinResponse {
        nonce_echo: parsed_request.nonce,
        status: STATUS_APPROVED,
        ciphertext,
    };
    let response_bytes = response.serialize();

    // 8. Device deserializes response
    let parsed_response = JoinResponse::deserialize(&response_bytes).unwrap();
    assert_eq!(parsed_response.nonce_echo, nonce);
    assert_eq!(parsed_response.status, STATUS_APPROVED);

    // 9. Device verifies nonce matches
    assert_eq!(parsed_response.nonce_echo, request.nonce);

    // 10. Device decrypts credentials
    let plaintext = crypto::seal_open(&parsed_response.ciphertext, &device_kp.secret).unwrap();
    let decrypted_creds = WifiCredentials::deserialize(&plaintext).unwrap();
    assert_eq!(decrypted_creds.ssid, "HomeWiFi");
    assert_eq!(decrypted_creds.security_type, SecurityType::Wpa2Psk);
    assert_eq!(decrypted_creds.credential, "super-secret-password");
}

/// Nonce mismatch rejection.
#[test]
fn nonce_mismatch_detected() {
    let request_nonce = [0x11; 8];
    let response_nonce = [0x22; 8];
    assert_ne!(request_nonce, response_nonce);

    let response = JoinResponse {
        nonce_echo: response_nonce,
        status: STATUS_APPROVED,
        ciphertext: vec![],
    };
    let bytes = response.serialize();
    let parsed = JoinResponse::deserialize(&bytes).unwrap();
    // Device should detect mismatch
    assert_ne!(parsed.nonce_echo, request_nonce);
}

/// Denied response has no ciphertext.
#[test]
fn denied_response_handling() {
    let response = JoinResponse {
        nonce_echo: [0x33; 8],
        status: STATUS_DENIED,
        ciphertext: vec![],
    };
    let bytes = response.serialize();
    let parsed = JoinResponse::deserialize(&bytes).unwrap();
    assert_eq!(parsed.status, STATUS_DENIED);
    assert!(parsed.ciphertext.is_empty());
}

/// Wrong key cannot decrypt sealed credentials.
#[test]
fn wrong_key_decryption_fails() {
    let device_kp = Keypair::generate();
    let attacker_kp = Keypair::generate();

    let credentials = WifiCredentials {
        ssid: String::from("SecureNet"),
        security_type: SecurityType::Wpa3Sae,
        credential: String::from("very-secure"),
    };
    let cred_bytes = credentials.serialize().unwrap();
    let recipient_pk = crypto_box::PublicKey::from(device_kp.public_bytes());
    let ciphertext = crypto::seal(&cred_bytes, &recipient_pk);

    // Attacker's key should fail to decrypt
    assert!(crypto::seal_open(&ciphertext, &attacker_kp.secret).is_none());

    // Device's key should succeed
    let plaintext = crypto::seal_open(&ciphertext, &device_kp.secret).unwrap();
    let decrypted = WifiCredentials::deserialize(&plaintext).unwrap();
    assert_eq!(decrypted.ssid, "SecureNet");
}

/// Verify code color derivation is consistent across both sides.
#[test]
fn color_derivation_consistency() {
    let kp = Keypair::generate();
    let pubkey = kp.public_bytes();

    // Both sides compute the same verify code
    let code1 = VerifyCode::from_pubkey(&pubkey);
    let code2 = VerifyCode::from_pubkey(&pubkey);
    assert_eq!(code1, code2);
    assert_eq!(code1.color(), code2.color());
    assert_eq!(code1.digit(), code2.digit());
}

/// Base64 encoding/decoding for OMI InfoItem pubkey transfer.
#[test]
fn base64_pubkey_omi_infoitem() {
    let kp = Keypair::generate();
    let pubkey = kp.public_bytes();

    // Encode for OMI InfoItem
    let encoded = base64_encode(&pubkey);
    // Decode back
    let decoded = base64_decode(&encoded).unwrap();
    assert_eq!(decoded, pubkey);
}

/// WPA2-Enterprise credentials round-trip through seal/open.
#[test]
fn enterprise_credentials_roundtrip() {
    let device_kp = Keypair::generate();
    let credentials = WifiCredentials {
        ssid: String::from("CorpNet"),
        security_type: SecurityType::Wpa2Enterprise,
        credential: String::from("user:cert-blob-here"),
    };
    let cred_bytes = credentials.serialize().unwrap();
    let recipient_pk = crypto_box::PublicKey::from(device_kp.public_bytes());
    let ciphertext = crypto::seal(&cred_bytes, &recipient_pk);
    let plaintext = crypto::seal_open(&ciphertext, &device_kp.secret).unwrap();
    let decrypted = WifiCredentials::deserialize(&plaintext).unwrap();
    assert_eq!(decrypted, credentials);
}

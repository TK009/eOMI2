// WSOP wire-format serialization — host-testable.
//
// JOIN_REQUEST: max 84 bytes, device → AP.
// JOIN_RESPONSE: variable length, AP → device.
//
// All multi-byte integers are big-endian. Strings are length-prefixed
// (1-byte length + UTF-8 bytes).

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use base64::{engine::general_purpose::STANDARD, Engine};

/// Current protocol version.
pub const PROTOCOL_VERSION: u8 = 0x01;

/// Maximum JOIN_REQUEST size in bytes.
/// version(1) + name(1+32) + mac(6) + pubkey(32) + nonce(8) + timestamp(4) = 84.
pub const MAX_JOIN_REQUEST: usize = 84;

/// JOIN_RESPONSE status: approved.
pub const STATUS_APPROVED: u8 = 0x01;
/// JOIN_RESPONSE status: denied.
pub const STATUS_DENIED: u8 = 0x00;

/// Security type for WiFi credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SecurityType {
    Wpa2Psk = 0x01,
    Wpa3Sae = 0x02,
    Wpa2Enterprise = 0x03,
}

impl SecurityType {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Wpa2Psk),
            0x02 => Some(Self::Wpa3Sae),
            0x03 => Some(Self::Wpa2Enterprise),
            _ => None,
        }
    }
}

/// JOIN_REQUEST message (device → AP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequest {
    /// Human-readable device name (max 32 bytes UTF-8).
    pub name: String,
    pub mac: [u8; 6],
    pub pubkey: [u8; 32],
    pub nonce: [u8; 8],
    pub timestamp: u32,
}

/// WiFi credentials (plaintext inside JOIN_RESPONSE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiCredentials {
    /// SSID (max 32 bytes UTF-8).
    pub ssid: String,
    pub security_type: SecurityType,
    /// Passphrase or credential blob (max 63 bytes UTF-8).
    pub credential: String,
}

/// JOIN_RESPONSE message (AP → device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinResponse {
    pub nonce_echo: [u8; 8],
    pub status: u8,
    pub ciphertext: Vec<u8>,
}

/// Serialization error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidVersion,
    TruncatedMessage,
    InvalidUtf8,
    FieldTooLong,
    InvalidSecurityType,
}

// ---------- Helpers ----------

fn write_len_prefixed_str(buf: &mut Vec<u8>, s: &str, max_len: usize) -> Result<(), ProtocolError> {
    let len = s.len();
    if len > max_len {
        return Err(ProtocolError::FieldTooLong);
    }
    buf.push(len as u8);
    buf.extend_from_slice(s.as_bytes());
    Ok(())
}

fn read_len_prefixed_str(data: &[u8], offset: &mut usize) -> Result<String, ProtocolError> {
    if *offset >= data.len() {
        return Err(ProtocolError::TruncatedMessage);
    }
    let len = data[*offset] as usize;
    *offset += 1;
    if *offset + len > data.len() {
        return Err(ProtocolError::TruncatedMessage);
    }
    let s = core::str::from_utf8(&data[*offset..*offset + len])
        .map_err(|_| ProtocolError::InvalidUtf8)?;
    *offset += len;
    Ok(String::from(s))
}

fn read_bytes<const N: usize>(
    data: &[u8],
    offset: &mut usize,
) -> Result<[u8; N], ProtocolError> {
    if *offset + N > data.len() {
        return Err(ProtocolError::TruncatedMessage);
    }
    let mut arr = [0u8; N];
    arr.copy_from_slice(&data[*offset..*offset + N]);
    *offset += N;
    Ok(arr)
}

// ---------- JoinRequest ----------

impl JoinRequest {
    /// Serialize to wire format (max 83 bytes).
    pub fn serialize(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut buf = Vec::with_capacity(MAX_JOIN_REQUEST);
        buf.push(PROTOCOL_VERSION);
        write_len_prefixed_str(&mut buf, &self.name, 32)?;
        buf.extend_from_slice(&self.mac);
        buf.extend_from_slice(&self.pubkey);
        buf.extend_from_slice(&self.nonce);
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        debug_assert!(buf.len() <= MAX_JOIN_REQUEST);
        Ok(buf)
    }

    /// Deserialize from wire format.
    pub fn deserialize(data: &[u8]) -> Result<Self, ProtocolError> {
        if data.is_empty() {
            return Err(ProtocolError::TruncatedMessage);
        }
        let mut offset = 0;

        let version = data[offset];
        offset += 1;
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidVersion);
        }

        let name = read_len_prefixed_str(data, &mut offset)?;
        let mac = read_bytes::<6>(data, &mut offset)?;
        let pubkey = read_bytes::<32>(data, &mut offset)?;
        let nonce = read_bytes::<8>(data, &mut offset)?;
        let ts_bytes = read_bytes::<4>(data, &mut offset)?;
        let timestamp = u32::from_be_bytes(ts_bytes);

        Ok(Self {
            name,
            mac,
            pubkey,
            nonce,
            timestamp,
        })
    }
}

// ---------- WifiCredentials ----------

impl WifiCredentials {
    /// Serialize credentials to plaintext bytes (for encryption).
    pub fn serialize(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut buf = Vec::with_capacity(97);
        write_len_prefixed_str(&mut buf, &self.ssid, 32)?;
        buf.push(self.security_type as u8);
        write_len_prefixed_str(&mut buf, &self.credential, 63)?;
        Ok(buf)
    }

    /// Deserialize credentials from decrypted plaintext.
    pub fn deserialize(data: &[u8]) -> Result<Self, ProtocolError> {
        let mut offset = 0;
        let ssid = read_len_prefixed_str(data, &mut offset)?;
        if offset >= data.len() {
            return Err(ProtocolError::TruncatedMessage);
        }
        let security_type =
            SecurityType::from_byte(data[offset]).ok_or(ProtocolError::InvalidSecurityType)?;
        offset += 1;
        let credential = read_len_prefixed_str(data, &mut offset)?;
        Ok(Self {
            ssid,
            security_type,
            credential,
        })
    }
}

// ---------- JoinResponse ----------

impl JoinResponse {
    /// Serialize to wire format.
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1 + 8 + 1 + self.ciphertext.len());
        buf.push(PROTOCOL_VERSION);
        buf.extend_from_slice(&self.nonce_echo);
        buf.push(self.status);
        buf.extend_from_slice(&self.ciphertext);
        buf
    }

    /// Deserialize from wire format.
    pub fn deserialize(data: &[u8]) -> Result<Self, ProtocolError> {
        if data.len() < 10 {
            return Err(ProtocolError::TruncatedMessage);
        }
        let mut offset = 0;

        let version = data[offset];
        offset += 1;
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidVersion);
        }

        let nonce_echo = read_bytes::<8>(data, &mut offset)?;
        let status = data[offset];
        offset += 1;
        let ciphertext = data[offset..].to_vec();

        Ok(Self {
            nonce_echo,
            status,
            ciphertext,
        })
    }
}

// ---------- Base64 helpers for OMI InfoItem values ----------

/// Encode bytes to base64 string for OMI InfoItem values.
pub fn base64_encode(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// Decode base64 string from OMI InfoItem values.
pub fn base64_decode(encoded: &str) -> Result<Vec<u8>, ProtocolError> {
    STANDARD
        .decode(encoded)
        .map_err(|_| ProtocolError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> JoinRequest {
        JoinRequest {
            name: String::from("test-device"),
            mac: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            pubkey: [1u8; 32],
            nonce: [2u8; 8],
            timestamp: 1700000000,
        }
    }

    #[test]
    fn join_request_roundtrip() {
        let req = sample_request();
        let bytes = req.serialize().unwrap();
        assert!(bytes.len() <= MAX_JOIN_REQUEST);
        let req2 = JoinRequest::deserialize(&bytes).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn join_request_max_name_length() {
        let mut req = sample_request();
        req.name = String::from("abcdefghijklmnopqrstuvwxyz012345"); // 32 chars
        let bytes = req.serialize().unwrap();
        assert_eq!(bytes.len(), MAX_JOIN_REQUEST);
        let req2 = JoinRequest::deserialize(&bytes).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn join_request_empty_name() {
        let mut req = sample_request();
        req.name = String::new();
        let bytes = req.serialize().unwrap();
        assert_eq!(bytes.len(), 1 + 1 + 6 + 32 + 8 + 4); // 52 bytes
        let req2 = JoinRequest::deserialize(&bytes).unwrap();
        assert_eq!(req, req2);
    }

    #[test]
    fn join_request_name_too_long() {
        let mut req = sample_request();
        req.name = "x".repeat(33);
        assert_eq!(req.serialize(), Err(ProtocolError::FieldTooLong));
    }

    #[test]
    fn join_request_invalid_version() {
        let mut bytes = sample_request().serialize().unwrap();
        bytes[0] = 0x99;
        assert_eq!(
            JoinRequest::deserialize(&bytes),
            Err(ProtocolError::InvalidVersion)
        );
    }

    #[test]
    fn join_request_truncated() {
        let bytes = sample_request().serialize().unwrap();
        assert_eq!(
            JoinRequest::deserialize(&bytes[..5]),
            Err(ProtocolError::TruncatedMessage)
        );
    }

    #[test]
    fn wifi_credentials_roundtrip() {
        let cred = WifiCredentials {
            ssid: String::from("MyNetwork"),
            security_type: SecurityType::Wpa2Psk,
            credential: String::from("password123"),
        };
        let bytes = cred.serialize().unwrap();
        let cred2 = WifiCredentials::deserialize(&bytes).unwrap();
        assert_eq!(cred, cred2);
    }

    #[test]
    fn wifi_credentials_all_security_types() {
        for st in [
            SecurityType::Wpa2Psk,
            SecurityType::Wpa3Sae,
            SecurityType::Wpa2Enterprise,
        ] {
            let cred = WifiCredentials {
                ssid: String::from("net"),
                security_type: st,
                credential: String::from("pass"),
            };
            let bytes = cred.serialize().unwrap();
            let cred2 = WifiCredentials::deserialize(&bytes).unwrap();
            assert_eq!(cred2.security_type, st);
        }
    }

    #[test]
    fn join_response_approved_roundtrip() {
        let resp = JoinResponse {
            nonce_echo: [3u8; 8],
            status: STATUS_APPROVED,
            ciphertext: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let bytes = resp.serialize();
        let resp2 = JoinResponse::deserialize(&bytes).unwrap();
        assert_eq!(resp, resp2);
    }

    #[test]
    fn join_response_denied_no_ciphertext() {
        let resp = JoinResponse {
            nonce_echo: [4u8; 8],
            status: STATUS_DENIED,
            ciphertext: vec![],
        };
        let bytes = resp.serialize();
        assert_eq!(bytes.len(), 10);
        let resp2 = JoinResponse::deserialize(&bytes).unwrap();
        assert_eq!(resp, resp2);
    }

    #[test]
    fn base64_roundtrip() {
        let data = b"Hello, WSOP!";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn base64_encode_pubkey() {
        let pubkey = [0xABu8; 32];
        let encoded = base64_encode(&pubkey);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, pubkey);
    }
}

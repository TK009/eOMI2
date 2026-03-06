// Lightweight DNS responder for captive portal redirect.
//
// Answers ALL A-record queries with the AP IP address so that any domain
// lookup resolves to the device, triggering OS/browser captive portal
// detection. Non-A queries get an empty (no-answer) response.
//
// Pure packet logic is platform-independent and testable on host.
// The ESP UDP listener is gated behind #[cfg(feature = "esp")].

/// DNS header length in bytes.
const HEADER_LEN: usize = 12;

/// Build a DNS response that answers an A query with `ip`.
///
/// Given the raw bytes of an incoming DNS query, produces a valid response:
/// - Copies the transaction ID from the query
/// - Sets QR=1 (response), AA=1 (authoritative), RD=1, RA=1
/// - Echoes the question section
/// - Appends one A record answer pointing to `ip`
///
/// Returns `None` if the query is too short to be valid DNS.
pub fn build_response(query: &[u8], ip: [u8; 4]) -> Option<Vec<u8>> {
    if query.len() < HEADER_LEN {
        return None;
    }

    // Find the end of the question section (skip QNAME + QTYPE + QCLASS).
    let q_end = skip_question(&query[HEADER_LEN..])?;
    let question_end = HEADER_LEN + q_end;

    let question_section = &query[HEADER_LEN..question_end];

    // Check QTYPE — only inject an answer for A records (type 1).
    // QTYPE is the 2 bytes right before QCLASS at the end of the question.
    let qtype = if question_section.len() >= 4 {
        u16::from_be_bytes([
            question_section[question_section.len() - 4],
            question_section[question_section.len() - 3],
        ])
    } else {
        0
    };

    let has_answer = qtype == 1; // A record
    let ancount: u16 = if has_answer { 1 } else { 0 };

    let answer_len = if has_answer { 16 } else { 0 }; // name-ptr(2) + type(2) + class(2) + ttl(4) + rdlen(2) + rdata(4)
    let resp_len = question_end + answer_len;
    let mut resp = Vec::with_capacity(resp_len);

    // --- Header (12 bytes) ---
    // Transaction ID from query
    resp.push(query[0]);
    resp.push(query[1]);
    // Flags: QR=1, AA=1, RD=1, RA=1 → 0x8580 + RD from query
    resp.push(0x85);
    resp.push(0x80);
    // QDCOUNT = 1
    resp.push(0x00);
    resp.push(0x01);
    // ANCOUNT
    resp.push((ancount >> 8) as u8);
    resp.push(ancount as u8);
    // NSCOUNT = 0
    resp.push(0x00);
    resp.push(0x00);
    // ARCOUNT = 0
    resp.push(0x00);
    resp.push(0x00);

    // --- Question section (echoed from query) ---
    resp.extend_from_slice(question_section);

    // --- Answer section (A record) ---
    if has_answer {
        // Name: pointer to offset 12 (start of question QNAME)
        resp.push(0xC0);
        resp.push(0x0C);
        // Type: A (1)
        resp.push(0x00);
        resp.push(0x01);
        // Class: IN (1)
        resp.push(0x00);
        resp.push(0x01);
        // TTL: 60 seconds (short — portal is temporary)
        resp.push(0x00);
        resp.push(0x00);
        resp.push(0x00);
        resp.push(0x3C);
        // RDLENGTH: 4
        resp.push(0x00);
        resp.push(0x04);
        // RDATA: IP address
        resp.extend_from_slice(&ip);
    }

    Some(resp)
}

/// Parse a dotted IP string like "192.168.4.1" into 4 bytes.
pub fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut parts = s.split('.');
    for octet in &mut octets {
        let part = parts.next()?;
        *octet = part.parse().ok()?;
    }
    if parts.next().is_some() {
        return None; // too many parts
    }
    Some(octets)
}

/// Skip one DNS question entry (QNAME + QTYPE + QCLASS).
/// Returns the number of bytes consumed, or `None` if malformed.
fn skip_question(data: &[u8]) -> Option<usize> {
    let mut pos = 0;
    // Walk the label sequence
    loop {
        if pos >= data.len() {
            return None;
        }
        let len = data[pos] as usize;
        if len == 0 {
            pos += 1; // null terminator
            break;
        }
        if len & 0xC0 == 0xC0 {
            // Compressed pointer — 2 bytes
            pos += 2;
            break;
        }
        pos += 1 + len;
    }
    // QTYPE (2) + QCLASS (2)
    pos += 4;
    if pos > data.len() {
        return None;
    }
    Some(pos)
}

#[cfg(feature = "esp")]
mod esp_impl {
    use log::{info, warn};
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// Handle for a running DNS responder. Drop or call `stop()` to shut down.
    pub struct DnsServer {
        running: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl DnsServer {
        /// Start the DNS responder on a background thread.
        ///
        /// Binds UDP port 53 on `bind_ip` (typically "0.0.0.0") and responds
        /// to every query with `redirect_ip`.
        pub fn start(bind_ip: &str, redirect_ip: &str) -> anyhow::Result<Self> {
            let ip_bytes = super::parse_ip(redirect_ip)
                .ok_or_else(|| anyhow::anyhow!("invalid redirect IP: {}", redirect_ip))?;

            let addr = format!("{}:53", bind_ip);
            let socket = UdpSocket::bind(&addr)?;
            socket.set_read_timeout(Some(Duration::from_millis(500)))?;

            let running = Arc::new(AtomicBool::new(true));
            let running_clone = running.clone();

            info!("DNS responder started on {}, redirecting to {}", addr, redirect_ip);

            let handle = std::thread::Builder::new()
                .name("dns-responder".into())
                .stack_size(4096)
                .spawn(move || {
                    let mut buf = [0u8; 512];
                    while running_clone.load(Ordering::Relaxed) {
                        let (len, src) = match socket.recv_from(&mut buf) {
                            Ok(r) => r,
                            Err(_) => continue, // timeout or transient error
                        };
                        if let Some(resp) = super::build_response(&buf[..len], ip_bytes) {
                            if let Err(e) = socket.send_to(&resp, src) {
                                warn!("DNS send error: {}", e);
                            }
                        }
                    }
                })?;

            Ok(Self {
                running,
                handle: Some(handle),
            })
        }

        /// Stop the DNS responder.
        pub fn stop(&mut self) {
            self.running.store(false, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    impl Drop for DnsServer {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(feature = "esp")]
pub use esp_impl::DnsServer;

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid DNS query for "example.com" type A class IN.
    fn example_query() -> Vec<u8> {
        let mut q = Vec::new();
        // Header
        q.extend_from_slice(&[
            0xAB, 0xCD, // Transaction ID
            0x01, 0x00, // Flags: standard query, RD=1
            0x00, 0x01, // QDCOUNT = 1
            0x00, 0x00, // ANCOUNT = 0
            0x00, 0x00, // NSCOUNT = 0
            0x00, 0x00, // ARCOUNT = 0
        ]);
        // Question: example.com, type A, class IN
        q.extend_from_slice(&[
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e',
            3, b'c', b'o', b'm',
            0, // null terminator
            0x00, 0x01, // QTYPE = A
            0x00, 0x01, // QCLASS = IN
        ]);
        q
    }

    /// DNS query with QTYPE=AAAA (28) for IPv6.
    fn aaaa_query() -> Vec<u8> {
        let mut q = Vec::new();
        q.extend_from_slice(&[
            0x12, 0x34, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ]);
        q.extend_from_slice(&[
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e',
            3, b'c', b'o', b'm',
            0,
            0x00, 0x1C, // QTYPE = AAAA (28)
            0x00, 0x01,
        ]);
        q
    }

    #[test]
    fn response_preserves_transaction_id() {
        let query = example_query();
        let resp = build_response(&query, [192, 168, 4, 1]).unwrap();
        assert_eq!(resp[0], 0xAB);
        assert_eq!(resp[1], 0xCD);
    }

    #[test]
    fn response_sets_flags() {
        let query = example_query();
        let resp = build_response(&query, [192, 168, 4, 1]).unwrap();
        // QR=1, AA=1, RD=1, RA=1
        assert_eq!(resp[2], 0x85);
        assert_eq!(resp[3], 0x80);
    }

    #[test]
    fn response_has_one_answer() {
        let query = example_query();
        let resp = build_response(&query, [192, 168, 4, 1]).unwrap();
        // QDCOUNT = 1
        assert_eq!(resp[4], 0x00);
        assert_eq!(resp[5], 0x01);
        // ANCOUNT = 1
        assert_eq!(resp[6], 0x00);
        assert_eq!(resp[7], 0x01);
    }

    #[test]
    fn response_contains_redirect_ip() {
        let ip = [192, 168, 4, 1];
        let query = example_query();
        let resp = build_response(&query, ip).unwrap();
        // Last 4 bytes should be the IP
        let len = resp.len();
        assert_eq!(&resp[len - 4..], &ip);
    }

    #[test]
    fn response_answer_has_correct_structure() {
        let query = example_query();
        let resp = build_response(&query, [10, 0, 0, 1]).unwrap();
        // Question section ends, then answer starts
        let q_section_len = query.len() - HEADER_LEN;
        let answer_start = HEADER_LEN + q_section_len;
        // Name pointer
        assert_eq!(resp[answer_start], 0xC0);
        assert_eq!(resp[answer_start + 1], 0x0C);
        // Type A
        assert_eq!(resp[answer_start + 2], 0x00);
        assert_eq!(resp[answer_start + 3], 0x01);
        // Class IN
        assert_eq!(resp[answer_start + 4], 0x00);
        assert_eq!(resp[answer_start + 5], 0x01);
        // TTL = 60
        assert_eq!(
            u32::from_be_bytes([
                resp[answer_start + 6],
                resp[answer_start + 7],
                resp[answer_start + 8],
                resp[answer_start + 9],
            ]),
            60
        );
        // RDLENGTH = 4
        assert_eq!(resp[answer_start + 10], 0x00);
        assert_eq!(resp[answer_start + 11], 0x04);
        // RDATA = IP
        assert_eq!(&resp[answer_start + 12..], &[10, 0, 0, 1]);
    }

    #[test]
    fn aaaa_query_gets_empty_response() {
        let query = aaaa_query();
        let resp = build_response(&query, [192, 168, 4, 1]).unwrap();
        // ANCOUNT = 0 (no answer for non-A queries)
        assert_eq!(resp[6], 0x00);
        assert_eq!(resp[7], 0x00);
        // Response should just be header + question, no answer section
        let expected_len = HEADER_LEN + (query.len() - HEADER_LEN);
        assert_eq!(resp.len(), expected_len);
    }

    #[test]
    fn rejects_too_short() {
        assert!(build_response(&[0; 5], [1, 2, 3, 4]).is_none());
        assert!(build_response(&[], [1, 2, 3, 4]).is_none());
    }

    #[test]
    fn rejects_truncated_question() {
        // Header only, no question data despite QDCOUNT=1
        let mut q = vec![0u8; HEADER_LEN];
        q[4] = 0x00;
        q[5] = 0x01; // QDCOUNT = 1
        assert!(build_response(&q, [1, 2, 3, 4]).is_none());
    }

    #[test]
    fn parse_ip_valid() {
        assert_eq!(parse_ip("192.168.4.1"), Some([192, 168, 4, 1]));
        assert_eq!(parse_ip("0.0.0.0"), Some([0, 0, 0, 0]));
        assert_eq!(parse_ip("255.255.255.255"), Some([255, 255, 255, 255]));
    }

    #[test]
    fn parse_ip_invalid() {
        assert_eq!(parse_ip(""), None);
        assert_eq!(parse_ip("192.168.4"), None);
        assert_eq!(parse_ip("192.168.4.1.5"), None);
        assert_eq!(parse_ip("abc.def.ghi.jkl"), None);
        assert_eq!(parse_ip("256.0.0.1"), None);
    }

    #[test]
    fn single_label_query() {
        let mut q = Vec::new();
        q.extend_from_slice(&[
            0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ]);
        // Single label: "local"
        q.extend_from_slice(&[5, b'l', b'o', b'c', b'a', b'l', 0]);
        q.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]); // A, IN
        let resp = build_response(&q, [192, 168, 4, 1]).unwrap();
        assert_eq!(resp[6], 0x00);
        assert_eq!(resp[7], 0x01); // ANCOUNT = 1
    }
}

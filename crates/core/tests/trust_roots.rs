//! `trust_roots`: extra CAs for self-hosted servers (a home CA, the phone's store).

#[test]
fn takes_good_certificates_and_skips_broken_ones() {
    let ca = include_bytes!("data/test-ca.der").to_vec();
    let broken = b"not a certificate".to_vec();
    assert_eq!(rc_core::trust_roots([ca, broken]), 1);
    // A client still builds with them.
    let _ = rc_core::Client::http();
}

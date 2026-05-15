//! In-memory SSL adapter and TLS session helpers.
//!
//! Android Auto TLS works differently from standard HTTPS: TLS records are
//! exchanged as payloads of `SslHandshake` AA frames rather than over a
//! raw TCP stream. This module provides:
//!
//! - [`BioAdapter`] — an in-memory `Read + Write` adapter that lets OpenSSL
//!   operate against byte buffers instead of a socket.
//! - [`TlsSession`] — type alias for the completed `SslStream<BioAdapter>`.
//! - [`load_or_generate_cert`] / [`build_ssl_context`] — certificate helpers.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::Path;

use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::ssl::{SslContext, SslMethod, SslStream, SslVerifyMode};
use openssl::x509::{X509NameBuilder, X509};

use aap_contracts::TransportError;

// ── BioAdapter ────────────────────────────────────────────────────────────────

/// In-memory I/O adapter that lets OpenSSL read from and write to byte buffers.
///
/// - Push bytes received from the network with [`BioAdapter::push`]; SSL reads
///   them via the [`Read`] impl.
/// - After SSL processes them it writes output; drain it with [`BioAdapter::drain`]
///   and send it over the network.
pub(crate) struct BioAdapter {
    /// Bytes awaiting consumption by SSL (received from the network).
    to_ssl: VecDeque<u8>,
    /// Bytes produced by SSL (to be sent over the network).
    from_ssl: Vec<u8>,
}

impl BioAdapter {
    /// Create an empty adapter.
    pub(crate) fn new() -> Self {
        Self {
            to_ssl: VecDeque::new(),
            from_ssl: Vec::new(),
        }
    }

    /// Feed bytes received from the network into the SSL read buffer.
    pub(crate) fn push(&mut self, data: &[u8]) {
        self.to_ssl.extend(data);
    }

    /// Drain bytes that SSL has generated (to be sent over the network).
    pub(crate) fn drain(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.from_ssl)
    }
}

impl Read for BioAdapter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.to_ssl.is_empty() {
            // Signal WANT_READ: no data available yet. SSL will return
            // HandshakeError::WouldBlock, letting us fetch more from the network.
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "BIO read buffer empty",
            ));
        }
        // VecDeque<u8> implements Read (stable since Rust 1.63).
        Read::read(&mut self.to_ssl, buf)
    }
}

impl Write for BioAdapter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.from_ssl.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ── TlsSession ────────────────────────────────────────────────────────────────

/// A completed TLS session backed by an in-memory [`BioAdapter`].
///
/// After the handshake, encrypt by writing to the session (output goes to
/// [`BioAdapter::from_ssl`]) and decrypt by pushing ciphertext to
/// [`BioAdapter::to_ssl`] then reading plaintext from the session.
pub(crate) type TlsSession = SslStream<BioAdapter>;

// ── Certificate helpers ───────────────────────────────────────────────────────

/// Load a TLS certificate from `server/certs/`, or generate an ephemeral one.
///
/// Looks for `server/certs/server.crt` and `server/certs/server.key` relative
/// to the current working directory. Falls back to a freshly generated
/// self-signed RSA-2048 certificate when the files are absent.
pub(crate) fn load_or_generate_cert() -> Result<(PKey<Private>, X509), TransportError> {
    let crt_path = Path::new("server/certs/server.crt");
    let key_path = Path::new("server/certs/server.key");

    if crt_path.exists() && key_path.exists() {
        let cert_pem =
            std::fs::read(crt_path).map_err(|e| TransportError::Tls(format!("read cert: {e}")))?;
        let key_pem =
            std::fs::read(key_path).map_err(|e| TransportError::Tls(format!("read key: {e}")))?;

        let cert = X509::from_pem(&cert_pem)
            .map_err(|e| TransportError::Tls(format!("parse cert: {e}")))?;
        let pkey = PKey::private_key_from_pem(&key_pem)
            .map_err(|e| TransportError::Tls(format!("parse key: {e}")))?;

        return Ok((pkey, cert));
    }

    generate_self_signed_cert()
}

/// Generate an ephemeral self-signed RSA-2048 / SHA-256 certificate.
fn generate_self_signed_cert() -> Result<(PKey<Private>, X509), TransportError> {
    let rsa = Rsa::generate(2048).map_err(|e| TransportError::Tls(format!("RSA generate: {e}")))?;
    let pkey =
        PKey::from_rsa(rsa).map_err(|e| TransportError::Tls(format!("PKey from RSA: {e}")))?;

    let mut name =
        X509NameBuilder::new().map_err(|e| TransportError::Tls(format!("X509Name: {e}")))?;
    name.append_entry_by_text("CN", "smartcar")
        .map_err(|e| TransportError::Tls(format!("set CN: {e}")))?;
    let name = name.build();

    let mut builder =
        X509::builder().map_err(|e| TransportError::Tls(format!("X509 builder: {e}")))?;
    builder
        .set_version(2)
        .map_err(|e| TransportError::Tls(format!("set_version: {e}")))?;

    let serial = BigNum::from_u32(1)
        .and_then(|b| b.to_asn1_integer())
        .map_err(|e| TransportError::Tls(format!("serial: {e}")))?;
    builder
        .set_serial_number(&serial)
        .map_err(|e| TransportError::Tls(format!("set_serial_number: {e}")))?;

    builder
        .set_subject_name(&name)
        .map_err(|e| TransportError::Tls(format!("set_subject_name: {e}")))?;
    builder
        .set_issuer_name(&name)
        .map_err(|e| TransportError::Tls(format!("set_issuer_name: {e}")))?;
    builder
        .set_pubkey(&pkey)
        .map_err(|e| TransportError::Tls(format!("set_pubkey: {e}")))?;

    let not_before =
        Asn1Time::days_from_now(0).map_err(|e| TransportError::Tls(format!("not_before: {e}")))?;
    let not_after = Asn1Time::days_from_now(3650)
        .map_err(|e| TransportError::Tls(format!("not_after: {e}")))?;
    builder
        .set_not_before(&not_before)
        .map_err(|e| TransportError::Tls(format!("set_not_before: {e}")))?;
    builder
        .set_not_after(&not_after)
        .map_err(|e| TransportError::Tls(format!("set_not_after: {e}")))?;

    builder
        .sign(&pkey, MessageDigest::sha256())
        .map_err(|e| TransportError::Tls(format!("sign: {e}")))?;

    Ok((pkey, builder.build()))
}

/// Build a TLS server [`SslContext`] for the Android Auto protocol.
///
/// - Acts as TLS **server** (the phone side holds the certificate).
/// - Client certificate verification is disabled: the head unit does not
///   present a certificate in the AA protocol.
pub(crate) fn build_ssl_context(
    pkey: &PKey<Private>,
    cert: &X509,
) -> Result<SslContext, TransportError> {
    let mut ctx = SslContext::builder(SslMethod::tls_server())
        .map_err(|e| TransportError::Tls(format!("SslContext::builder: {e}")))?;

    ctx.set_verify(SslVerifyMode::NONE);

    ctx.set_private_key(pkey)
        .map_err(|e| TransportError::Tls(format!("set_private_key: {e}")))?;
    ctx.set_certificate(cert)
        .map_err(|e| TransportError::Tls(format!("set_certificate: {e}")))?;
    ctx.check_private_key()
        .map_err(|e| TransportError::Tls(format!("check_private_key: {e}")))?;

    Ok(ctx.build())
}

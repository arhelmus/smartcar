//! In-memory SSL adapter and TLS session helpers.
//!
//! Android Auto TLS works differently from standard HTTPS: TLS records are
//! exchanged as payloads of `SslHandshake` AA frames rather than over a
//! raw TCP stream. This module provides:
//!
//! - [`BioAdapter`] — an in-memory `Read + Write` adapter that lets OpenSSL
//!   operate against byte buffers instead of a socket.
//! - [`TlsSession`] — type alias for the completed `SslStream<BioAdapter>`.
//! - [`build_ssl_server_context`] — TLS server context for the phone side.
//!
//! In the Android Auto protocol the **phone is the TLS server**: the head unit
//! (openauto) initiates TLS as the client and sends ClientHello first. The
//! phone generates an ephemeral self-signed certificate; the head unit does
//! not verify it.

use std::collections::VecDeque;
use std::io::{self, Read, Write};

use openssl::asn1::Asn1Time;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use openssl::ssl::{SslContext, SslMethod, SslOptions, SslStream, SslVerifyMode};
use openssl::x509::{X509, X509NameBuilder};

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

// ── TLS server context ────────────────────────────────────────────────────────

/// Generate an ephemeral RSA 2048-bit self-signed certificate and key.
///
/// Android Auto does not verify the phone's certificate, so a fresh ephemeral
/// cert is fine and avoids any on-disk key-management complexity.
fn generate_self_signed_cert() -> Result<(X509, PKey<openssl::pkey::Private>), TransportError> {
    let rsa = Rsa::generate(2048)
        .map_err(|e| TransportError::Tls(format!("RSA keygen: {e}")))?;
    let key = PKey::from_rsa(rsa)
        .map_err(|e| TransportError::Tls(format!("PKey::from_rsa: {e}")))?;

    let mut name = X509NameBuilder::new()
        .map_err(|e| TransportError::Tls(format!("X509NameBuilder: {e}")))?;
    name.append_entry_by_text("CN", "Smartcar AA")
        .map_err(|e| TransportError::Tls(format!("X509Name append CN: {e}")))?;
    let name = name.build();

    let mut builder = X509::builder()
        .map_err(|e| TransportError::Tls(format!("X509::builder: {e}")))?;
    builder
        .set_version(2)
        .map_err(|e| TransportError::Tls(format!("set_version: {e}")))?;
    builder
        .set_subject_name(&name)
        .map_err(|e| TransportError::Tls(format!("set_subject_name: {e}")))?;
    builder
        .set_issuer_name(&name)
        .map_err(|e| TransportError::Tls(format!("set_issuer_name: {e}")))?;
    builder
        .set_pubkey(&key)
        .map_err(|e| TransportError::Tls(format!("set_pubkey: {e}")))?;

    let not_before = Asn1Time::days_from_now(0)
        .map_err(|e| TransportError::Tls(format!("Asn1Time not_before: {e}")))?;
    let not_after = Asn1Time::days_from_now(3650)
        .map_err(|e| TransportError::Tls(format!("Asn1Time not_after: {e}")))?;
    builder
        .set_not_before(&not_before)
        .map_err(|e| TransportError::Tls(format!("set_not_before: {e}")))?;
    builder
        .set_not_after(&not_after)
        .map_err(|e| TransportError::Tls(format!("set_not_after: {e}")))?;

    builder
        .sign(&key, MessageDigest::sha256())
        .map_err(|e| TransportError::Tls(format!("X509 sign: {e}")))?;

    Ok((builder.build(), key))
}

/// Build a TLS **server** [`SslContext`] for the Android Auto phone side.
///
/// The head unit (openauto) is the TLS client: it initiates the handshake by
/// sending ClientHello. The phone acts as TLS server with an ephemeral
/// self-signed certificate. The head unit does not verify the cert.
pub(crate) fn build_ssl_server_context() -> Result<SslContext, TransportError> {
    let (cert, key) = generate_self_signed_cert()?;

    let mut ctx = SslContext::builder(SslMethod::tls_server())
        .map_err(|e| TransportError::Tls(format!("SslContext::builder: {e}")))?;
    ctx.set_verify(SslVerifyMode::NONE);
    // Force TLS 1.2 maximum. TLS 1.3 sends a post-handshake NewSessionTicket
    // that openauto receives as a spurious SslHandshake frame, corrupting the
    // SSL BIO state before the first encrypted AA message arrives.
    // AACS (another AA head-unit implementation) applies the same restriction.
    ctx.set_options(SslOptions::NO_TLSV1_3);
    ctx.set_certificate(&cert)
        .map_err(|e| TransportError::Tls(format!("set_certificate: {e}")))?;
    ctx.set_private_key(&key)
        .map_err(|e| TransportError::Tls(format!("set_private_key: {e}")))?;
    Ok(ctx.build())
}

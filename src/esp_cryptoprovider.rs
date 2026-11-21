use defmt::{error, info};
use embedded_tls::{Aes128GcmSha256, Certificate, CertificateRef, CertificateVerifyRef, CryptoProvider, TlsCipherSuite, TlsError, TlsVerifier, pki::CertVerifier};
use esp_hal::rng::Trng;
use rand_core::{CryptoRng, CryptoRngCore, RngCore};

use crate::rtc::RtcClock;

pub struct EspRng(pub Trng);

impl RngCore for EspRng {
    fn next_u32(&mut self) -> u32 {
        self.0.random()
    }

    fn next_u64(&mut self) -> u64 {
        ((self.0.random() as u64) << 32) | (self.0.random() as u64)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.read(dest);
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0.read(dest);
        Ok(())
    }
}

impl CryptoRng for EspRng {}

pub struct EspCryptoProvider<'d, const CERT_SIZE: usize> {
    rng: EspRng,
    verifier: LoggingVerifier<CertVerifier<'d, Aes128GcmSha256, RtcClock, CERT_SIZE>>,
}

impl<'d, const CERT_SIZE: usize> EspCryptoProvider<'d, CERT_SIZE> {
    pub fn new(trng: Trng, root_ca_der: &'d [u8]) -> Self {
        Self {
            rng: EspRng(trng),
            verifier: LoggingVerifier::new(CertVerifier::new(Certificate::X509(root_ca_der))),
        }
    }
}

impl<'d, const CERT_SIZE: usize> CryptoProvider for EspCryptoProvider<'d, CERT_SIZE> {
    type CipherSuite = Aes128GcmSha256;
    // Check `Aes128GcmSha256` or `signature` crate bounds if `signer()` is needed;
    // `[u8; 64]` or `[u8; 72]` is common for ECDSA P-256 DER/raw signatures.
    type Signature = [u8; 72];

    fn rng(&mut self) -> impl CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }
}

pub struct LoggingVerifier<V> {
    inner: V,
}

impl<V> LoggingVerifier<V> {
    pub fn new(inner: V) -> Self {
        Self { inner }
    }
}

impl<C, V> TlsVerifier<C> for LoggingVerifier<V>
where
    C: TlsCipherSuite,
    V: TlsVerifier<C>,
{
    fn set_hostname_verification(&mut self, hostname: &str) -> Result<(), TlsError> {
        let res = self.inner.set_hostname_verification(hostname);
        match res {
            Ok(()) => info!("TLS hostname verification succeeded"),
            Err(err) => error!("TLS hostname verification failed: {}", err),
        }
        res
    }

    fn verify_certificate(
        &mut self,
        transcript: &C::Hash,
        cert: CertificateRef<'_>,
    ) -> Result<(), TlsError> {
        let res = self.inner.verify_certificate(transcript, cert);
        match res {
            Ok(()) => info!("TLS server certificate verified against CA"),
            Err(err) => error!("TLS certificate verification failed: {}", err),
        }
        res
    }

    fn verify_signature(&mut self, verify: CertificateVerifyRef<'_>) -> Result<(), TlsError> {
        let res = self.inner.verify_signature(verify);
        match res {
            Ok(()) => info!("TLS server handshake signature verified"),
            Err(err) => error!("TLS signature verification failed: {}", err),
        }
        res
    }
}
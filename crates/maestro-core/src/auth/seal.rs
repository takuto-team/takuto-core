// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
#![allow(deprecated)] // Transitional: ConfigStr sites rewritten to ConfigError variants in C2.

//! Envelope encryption with XChaCha20-Poly1305. See module docs on `auth::mod`
//! for the high-level scheme.

use chacha20poly1305::{
    AeadCore, KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, OsRng},
};
use zeroize::Zeroizing;

use crate::auth::master_key::MasterKey;
use crate::error::{MaestroError, Result};

/// A sealed credential row. Lives in the database; every byte is safe to
/// store on cold disk because only the holder of the master key can open it.
#[derive(Debug, Clone)]
pub struct SealedBlob {
    /// AEAD-sealed plaintext: ciphertext + 16-byte Poly1305 tag.
    pub ciphertext: Vec<u8>,
    /// Fresh 24-byte nonce used to seal the plaintext.
    pub nonce: [u8; 24],
    /// AEAD-sealed DEK (32 bytes + 16-byte tag).
    pub wrapped_dek: Vec<u8>,
    /// Fresh 24-byte nonce used to seal the DEK.
    pub wnonce: [u8; 24],
}

/// Seal `plaintext` under a fresh per-row DEK; wrap the DEK with `master`.
///
/// Both nonces are CSPRNG-generated. Returns `Err` only on the (very rare)
/// case where the AEAD primitive itself reports a length / capacity failure;
/// the OS entropy source is consulted via `OsRng` and is considered infallible
/// on every supported platform.
pub fn seal(master: &MasterKey, plaintext: &[u8]) -> Result<SealedBlob> {
    // Fresh DEK per row.
    let dek: Zeroizing<[u8; 32]> = Zeroizing::new({
        let mut buf = [0u8; 32];
        getrandom::fill(&mut buf).map_err(|e| {
            MaestroError::ConfigStr(format!("CSPRNG failure while generating DEK: {e}"))
        })?;
        buf
    });

    // Seal plaintext with DEK + fresh nonce.
    let dek_cipher = XChaCha20Poly1305::new((&*dek).into());
    let nonce_arr: XNonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let nonce: [u8; 24] = nonce_arr.into();
    let ciphertext = dek_cipher
        .encrypt(&nonce_arr, plaintext)
        .map_err(|e| MaestroError::ConfigStr(format!("AEAD encrypt(plaintext) failed: {e}")))?;

    // Wrap the DEK with the master key + a separate fresh nonce.
    let mk_cipher = XChaCha20Poly1305::new((master.as_bytes()).into());
    let wnonce_arr: XNonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let wnonce: [u8; 24] = wnonce_arr.into();
    let wrapped_dek = mk_cipher
        .encrypt(&wnonce_arr, &dek[..])
        .map_err(|e| MaestroError::ConfigStr(format!("AEAD encrypt(DEK) failed: {e}")))?;

    // `dek` zeroizes on drop.
    Ok(SealedBlob {
        ciphertext,
        nonce,
        wrapped_dek,
        wnonce,
    })
}

/// Reverse of [`seal`]. AEAD tag failure (tampering, wrong master key, wrong
/// nonce length) → typed error.
pub fn open(master: &MasterKey, sealed: &SealedBlob) -> Result<Vec<u8>> {
    // Unwrap the DEK with the master key.
    let mk_cipher = XChaCha20Poly1305::new((master.as_bytes()).into());
    let wnonce: XNonce = sealed.wnonce.into();
    let dek_raw = mk_cipher
        .decrypt(&wnonce, sealed.wrapped_dek.as_slice())
        .map_err(|_| {
            MaestroError::ConfigStr(
                "envelope decrypt failed: wrapped DEK rejected (tampered ciphertext or wrong master key)"
                    .to_string(),
            )
        })?;
    if dek_raw.len() != 32 {
        return Err(MaestroError::ConfigStr(format!(
            "wrapped DEK decrypted to wrong length: {} (expected 32)",
            dek_raw.len()
        )));
    }
    let mut dek_bytes = [0u8; 32];
    dek_bytes.copy_from_slice(&dek_raw);
    let dek: Zeroizing<[u8; 32]> = Zeroizing::new(dek_bytes);
    // The intermediate `dek_raw` is a Vec<u8> we cannot zeroize directly;
    // drop it explicitly to shorten its lifetime, even though Vec doesn't
    // guarantee the buffer isn't realloced/copied earlier.
    drop(dek_raw);

    let dek_cipher = XChaCha20Poly1305::new((&*dek).into());
    let nonce: XNonce = sealed.nonce.into();
    let plaintext = dek_cipher
        .decrypt(&nonce, sealed.ciphertext.as_slice())
        .map_err(|_| {
            MaestroError::ConfigStr(
                "envelope decrypt failed: payload rejected (tampered ciphertext or wrong DEK)"
                    .to_string(),
            )
        })?;
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_master() -> MasterKey {
        // Deterministic master key for tests — never used in production.
        MasterKey::from_bytes([0xABu8; 32])
    }

    #[test]
    fn seal_then_open_round_trip_matches_plaintext() {
        let mk = fixed_master();
        let pt = b"secret-cursor-api-key-very-long-value-1234567890";
        let sealed = seal(&mk, pt).expect("seal");

        // Sanity: nonces are non-zero (fresh random).
        assert_ne!(sealed.nonce, [0u8; 24]);
        assert_ne!(sealed.wnonce, [0u8; 24]);
        assert_ne!(sealed.nonce, sealed.wnonce);
        // Ciphertext carries the 16-byte AEAD tag on top of the plaintext.
        assert_eq!(sealed.ciphertext.len(), pt.len() + 16);
        // wrapped_dek = 32-byte DEK + 16-byte tag.
        assert_eq!(sealed.wrapped_dek.len(), 32 + 16);

        let opened = open(&mk, &sealed).expect("open");
        assert_eq!(opened, pt);
    }

    #[test]
    fn open_with_wrong_master_key_fails() {
        let mk = fixed_master();
        let wrong = MasterKey::from_bytes([0x55u8; 32]);
        let pt = b"hello";
        let sealed = seal(&mk, pt).unwrap();

        let err = open(&wrong, &sealed).expect_err("wrong key must reject");
        assert!(
            err.to_string().contains("wrapped DEK rejected"),
            "got: {err}"
        );
    }

    #[test]
    fn tampered_ciphertext_fails_to_open() {
        let mk = fixed_master();
        let pt = b"hello world";
        let mut sealed = seal(&mk, pt).unwrap();
        // Flip a byte in the AEAD-sealed payload.
        sealed.ciphertext[0] ^= 0xFF;

        let err = open(&mk, &sealed).expect_err("tampered ct must reject");
        assert!(err.to_string().contains("payload rejected"), "got: {err}");
    }

    #[test]
    fn tampered_wrapped_dek_fails_to_open() {
        let mk = fixed_master();
        let pt = b"hello world";
        let mut sealed = seal(&mk, pt).unwrap();
        sealed.wrapped_dek[0] ^= 0xFF;

        let err = open(&mk, &sealed).expect_err("tampered wrapped_dek must reject");
        assert!(
            err.to_string().contains("wrapped DEK rejected"),
            "got: {err}"
        );
    }

    #[test]
    fn two_seals_of_same_plaintext_produce_different_blobs() {
        // Fresh DEK + fresh nonces per call mean the bytes diverge even though
        // the plaintext and master key are identical.
        let mk = fixed_master();
        let pt = b"identical";
        let a = seal(&mk, pt).unwrap();
        let b = seal(&mk, pt).unwrap();
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(a.wrapped_dek, b.wrapped_dek);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.wnonce, b.wnonce);
        // Both still decrypt to the same plaintext.
        assert_eq!(open(&mk, &a).unwrap(), pt);
        assert_eq!(open(&mk, &b).unwrap(), pt);
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let mk = fixed_master();
        let sealed = seal(&mk, b"").unwrap();
        let opened = open(&mk, &sealed).unwrap();
        assert_eq!(opened, b"");
    }
}

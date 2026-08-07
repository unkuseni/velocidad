//! Wallet cryptography:
//! - EVM address derivation from a secp256k1 keypair (`k256` + keccak-256)
//! - Private keys encrypted at rest with AES-256-GCM under a master key

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use k256::ecdsa::SigningKey;
use rand::RngCore;
use sha3::{Digest, Keccak256};

use crate::config::Config;

/// A generated or loaded EVM keypair.
pub struct GeneratedWallet {
    pub address: String,
    /// 32-byte private key, hex-encoded (no `0x` prefix).
    pub private_key_hex: String,
}

/// Deterministically derive an EVM address from a 32-byte secret key.
pub fn derive_address(secret: &[u8; 32]) -> Result<String> {
    let sk = SigningKey::from_bytes(secret.into()).context("invalid private key")?;
    let pk = sk.verifying_key().to_encoded_point(false);
    let pubkey = pk.as_bytes(); // 0x04 || X || Y
    let hash = Keccak256::digest(&pubkey[1..]);
    Ok(format!("0x{}", hex::encode(&hash[12..])))
}

/// Generate a fresh random keypair.
pub fn generate_wallet() -> Result<GeneratedWallet> {
    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let address = derive_address(&secret)?;
    Ok(GeneratedWallet {
        address,
        private_key_hex: hex::encode(secret),
    })
}

/// Import a keypair from a hex-encoded private key (with or without `0x`).
pub fn import_wallet(private_key_hex: &str) -> Result<GeneratedWallet> {
    let cleaned = private_key_hex.trim().strip_prefix("0x").unwrap_or(private_key_hex.trim());
    let bytes = hex::decode(cleaned).context("private key must be 64 hex characters")?;
    if bytes.len() != 32 {
        bail!("private key must be exactly 32 bytes (64 hex characters)");
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&bytes);
    let address = derive_address(&secret)?;
    Ok(GeneratedWallet {
        address,
        private_key_hex: hex::encode(secret),
    })
}

/// Manages the AES-256 master key used to encrypt wallet private keys.
///
/// Resolution order:
/// 1. `MASTER_KEY` env / config (64 hex chars)
/// 2. existing `./velocidad.key` file
/// 3. generate a fresh key and persist it to `./velocidad.key`
pub struct Keyring {
    key: [u8; 32],
}

impl Keyring {
    pub fn load(config: &Config) -> Result<Self> {
        let key = if let Some(hex_key) = &config.master_key {
            let bytes = hex::decode(hex_key)
                .map_err(|_| anyhow!("MASTER_KEY must be 64 hex characters"))?;
            if bytes.len() != 32 {
                bail!("MASTER_KEY must be exactly 32 bytes (64 hex characters)");
            }
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            k
        } else {
            match std::fs::read("velocidad.key") {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut k = [0u8; 32];
                    k.copy_from_slice(&bytes);
                    k
                }
                Ok(_) => bail!("velocidad.key exists but is not 32 bytes; delete it or set MASTER_KEY"),
                Err(_) => {
                    let mut k = [0u8; 32];
                    rand::rngs::OsRng.fill_bytes(&mut k);
                    std::fs::write("velocidad.key", k)
                        .context("failed to persist generated master key to velocidad.key")?;
                    tracing::warn!(
                        "generated new master key at ./velocidad.key — keep this file safe, \
                         wallet private keys are encrypted with it"
                    );
                    k
                }
            }
        };
        Ok(Self { key })
    }

    /// Encrypt plaintext → hex(nonce || ciphertext).
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<String> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| anyhow!("encryption failed"))?;
        let mut blob = nonce.as_slice().to_vec();
        blob.extend_from_slice(&ciphertext);
        Ok(hex::encode(blob))
    }

    /// Decrypt hex(nonce || ciphertext) → plaintext.
    ///
    /// Used when signing live transactions with a stored wallet key.
    #[allow(dead_code)]
    pub fn decrypt(&self, data: &str) -> Result<Vec<u8>> {
        let blob = hex::decode(data).context("malformed encrypted blob")?;
        if blob.len() < 12 {
            bail!("encrypted blob too short");
        }
        let (nonce_bytes, ciphertext) = blob.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let cipher = Aes256Gcm::new_from_slice(&self.key)?;
        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow!("decryption failed (wrong master key?)"))
    }
}

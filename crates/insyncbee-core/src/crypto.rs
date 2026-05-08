//! Per-sync-pair file encryption.
//!
//! Files uploaded for an encrypted pair are wrapped in this module's
//! streaming AEAD format before being handed to the Drive client, and
//! unwrapped on the way back out. The format is chunked so the existing
//! resumable-upload pipeline (which only ever holds one chunk in RAM) keeps
//! working for multi-GB files.
//!
//! ## On-disk format
//!
//! ```text
//! magic       8 bytes  "INSYNCBE"
//! version     1 byte   0x01
//! plaintext   8 bytes  big-endian total plaintext length
//! nonce_pre   7 bytes  random per-file nonce prefix
//! ── header ends at offset 24 ──
//! repeat for each plaintext chunk:
//!   ciphertext  N bytes (= chunk plaintext length)
//!   tag         16 bytes (GCM auth tag)
//! ```
//!
//! `PLAINTEXT_CHUNK = 64 KiB`. The last chunk may be shorter.
//!
//! Per-chunk nonce is `nonce_prefix || u32_be(chunk_idx) || last_flag`.
//! `last_flag = 0x01` for the final chunk (whose length may be less than
//! `PLAINTEXT_CHUNK`), `0x00` otherwise. Embedding the flag in the nonce
//! turns truncation attacks (chopping off chunks) into AEAD failures.
//!
//! ## Key handling
//!
//! Keys are 32 bytes derived from a user passphrase via Argon2id with a
//! per-pair random salt (stored in the DB). Once derived, the raw key
//! lives in the OS keyring (see [`crate::keystore`]) so the daemon can
//! sync without re-prompting.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::Argon2;
use rand::RngCore;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Plaintext chunk size. 64 KiB is small enough to keep memory low and
/// large enough that the per-chunk tag overhead (16 bytes) is negligible.
const PLAINTEXT_CHUNK: usize = 64 * 1024;

/// AES-GCM auth tag length in bytes.
const TAG_BYTES: usize = 16;

/// Ciphertext chunk = plaintext + tag.
const CIPHERTEXT_CHUNK: usize = PLAINTEXT_CHUNK + TAG_BYTES;

const MAGIC: &[u8; 8] = b"INSYNCBE";
const VERSION: u8 = 0x01;
const NONCE_PREFIX_LEN: usize = 7;
const HEADER_LEN: usize = 8 + 1 + 8 + NONCE_PREFIX_LEN; // 24

/// Argon2id parameters. Tuned for ~250ms on a modern laptop. We don't
/// expose these as knobs — the salt is per-pair, so a future rotation can
/// just bump VERSION and re-derive.
const ARGON2_M_COST: u32 = 64 * 1024; // 64 MiB
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 1;

/// Length of the per-pair Argon2 salt. 16 bytes is the recommended minimum.
pub const SALT_LEN: usize = 16;

/// Length of the derived encryption key.
pub const KEY_LEN: usize = 32;

/// Cipher state for a single sync pair. Holds the derived 32-byte key and
/// pre-built AES-GCM instance. The key is wiped on drop.
#[derive(ZeroizeOnDrop)]
pub struct FileCipher {
    key: [u8; KEY_LEN],
}

impl FileCipher {
    /// Wrap a raw 32-byte key (e.g. one just loaded from the OS keyring).
    pub fn from_key(key: [u8; KEY_LEN]) -> Self {
        Self { key }
    }

    /// Derive a key from a user passphrase + per-pair salt and wrap it.
    pub fn from_passphrase(passphrase: &str, salt: &[u8]) -> anyhow::Result<Self> {
        let key = derive_key(passphrase, salt)?;
        Ok(Self::from_key(key))
    }

    fn aead(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key))
    }

    /// Borrow the raw key bytes — only used by the keystore module to
    /// hand them to the OS keyring. Keep this crate-visible.
    pub(crate) fn key_bytes(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    /// Encrypt `plaintext_path` to `ciphertext_path` in the streaming
    /// chunked format. Reads/writes in 64 KiB blocks so multi-GB files
    /// don't OOM.
    pub async fn encrypt_file(
        &self,
        plaintext_path: &Path,
        ciphertext_path: &Path,
    ) -> anyhow::Result<()> {
        let aead = self.aead();
        let nonce_prefix = random_nonce_prefix();
        let total_size = tokio::fs::metadata(plaintext_path).await?.len();

        let in_file = File::open(plaintext_path).await?;
        let mut reader = BufReader::with_capacity(PLAINTEXT_CHUNK, in_file);

        if let Some(parent) = ciphertext_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let out_file = File::create(ciphertext_path).await?;
        let mut writer = BufWriter::with_capacity(CIPHERTEXT_CHUNK, out_file);

        // ── header ────────────────────────────────────────────────
        let mut header = [0u8; HEADER_LEN];
        header[0..8].copy_from_slice(MAGIC);
        header[8] = VERSION;
        header[9..17].copy_from_slice(&total_size.to_be_bytes());
        header[17..24].copy_from_slice(&nonce_prefix);
        writer.write_all(&header).await?;

        // ── chunks ────────────────────────────────────────────────
        let mut buf = vec![0u8; PLAINTEXT_CHUNK];
        let mut bytes_consumed: u64 = 0;
        let mut chunk_idx: u32 = 0;

        loop {
            let filled = read_full(&mut reader, &mut buf).await?;
            let is_last = bytes_consumed + filled as u64 >= total_size;
            let nonce = build_nonce(&nonce_prefix, chunk_idx, is_last);
            let aad = chunk_aad(chunk_idx, is_last);

            let ciphertext = aead
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &buf[..filled],
                        aad: &aad,
                    },
                )
                .map_err(|e| anyhow::anyhow!("AES-GCM encrypt failed: {e}"))?;
            writer.write_all(&ciphertext).await?;

            bytes_consumed += filled as u64;
            chunk_idx = chunk_idx.checked_add(1).ok_or_else(|| {
                anyhow::anyhow!("encrypt: chunk counter overflow (>4 billion chunks)")
            })?;

            if is_last {
                break;
            }
        }

        writer.flush().await?;
        buf.zeroize();
        Ok(())
    }

    /// Decrypt `ciphertext_path` to `plaintext_path`. Returns an error on
    /// any AEAD failure (truncation, re-ordering, wrong key, corruption).
    pub async fn decrypt_file(
        &self,
        ciphertext_path: &Path,
        plaintext_path: &Path,
    ) -> anyhow::Result<()> {
        let aead = self.aead();

        let in_file = File::open(ciphertext_path).await?;
        let mut reader = BufReader::with_capacity(CIPHERTEXT_CHUNK, in_file);

        // ── header ────────────────────────────────────────────────
        let mut header = [0u8; HEADER_LEN];
        reader.read_exact(&mut header).await?;
        if &header[0..8] != MAGIC {
            anyhow::bail!("decrypt: bad magic — not an InSyncBee encrypted file");
        }
        if header[8] != VERSION {
            anyhow::bail!(
                "decrypt: unsupported file version {} (this build supports v{VERSION})",
                header[8]
            );
        }
        let total_size = u64::from_be_bytes(header[9..17].try_into().unwrap());
        let mut nonce_prefix = [0u8; NONCE_PREFIX_LEN];
        nonce_prefix.copy_from_slice(&header[17..24]);

        if let Some(parent) = plaintext_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let out_file = File::create(plaintext_path).await?;
        let mut writer = BufWriter::with_capacity(PLAINTEXT_CHUNK, out_file);

        // ── chunks ────────────────────────────────────────────────
        let mut buf = vec![0u8; CIPHERTEXT_CHUNK];
        let mut bytes_emitted: u64 = 0;
        let mut chunk_idx: u32 = 0;

        loop {
            // The tail of the file may be shorter than a full chunk; that's
            // expected for the final chunk. Anything else (short read in
            // the middle, missing tag) is fatal.
            let read = read_full(&mut reader, &mut buf).await?;
            if read == 0 {
                anyhow::bail!(
                    "decrypt: unexpected end of file (got {bytes_emitted}/{total_size} bytes plaintext)"
                );
            }
            if read < TAG_BYTES {
                anyhow::bail!("decrypt: chunk shorter than auth tag");
            }
            let plaintext_len = read - TAG_BYTES;
            let projected = bytes_emitted + plaintext_len as u64;
            let is_last = projected >= total_size;
            let nonce = build_nonce(&nonce_prefix, chunk_idx, is_last);
            let aad = chunk_aad(chunk_idx, is_last);

            let plaintext = aead
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &buf[..read],
                        aad: &aad,
                    },
                )
                .map_err(|_| {
                    anyhow::anyhow!(
                        "decrypt: AEAD failure on chunk {chunk_idx} (wrong key or corrupted file)"
                    )
                })?;
            writer.write_all(&plaintext).await?;
            bytes_emitted += plaintext.len() as u64;
            chunk_idx = chunk_idx.checked_add(1).ok_or_else(|| {
                anyhow::anyhow!("decrypt: chunk counter overflow")
            })?;

            if is_last {
                break;
            }
        }

        if bytes_emitted != total_size {
            anyhow::bail!(
                "decrypt: size mismatch (header said {total_size}, decrypted {bytes_emitted})"
            );
        }

        writer.flush().await?;
        buf.zeroize();
        Ok(())
    }

    /// Build a small "key verifier" blob that proves whoever holds it knows
    /// the right key — without touching any real file. We store this in the
    /// DB so the UI can validate a passphrase before kicking off a sync.
    ///
    /// Format: `nonce(12) || ciphertext || tag`. Plaintext is the literal
    /// bytes of `expected`, which the caller passes back to `verify`.
    pub fn make_verifier(&self, expected: &[u8]) -> anyhow::Result<Vec<u8>> {
        let aead = self.aead();
        let nonce = Aes256Gcm::generate_nonce(&mut rand::thread_rng());
        let ciphertext = aead
            .encrypt(&nonce, expected)
            .map_err(|e| anyhow::anyhow!("verifier encrypt failed: {e}"))?;
        let mut out = Vec::with_capacity(12 + ciphertext.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt a verifier blob produced by [`make_verifier`] and return
    /// `Ok(true)` iff the plaintext matches `expected`. AEAD failures are
    /// returned as `Ok(false)` — the caller treats both as "wrong key".
    pub fn verify(&self, expected: &[u8], blob: &[u8]) -> anyhow::Result<bool> {
        if blob.len() < 12 + TAG_BYTES {
            return Ok(false);
        }
        let aead = self.aead();
        let nonce = Nonce::from_slice(&blob[..12]);
        let plaintext = match aead.decrypt(nonce, &blob[12..]) {
            Ok(p) => p,
            Err(_) => return Ok(false),
        };
        Ok(plaintext == expected)
    }
}

/// Generate a random per-pair Argon2 salt. Caller stores it alongside the
/// sync pair so the same passphrase always derives the same key.
pub fn random_salt() -> [u8; SALT_LEN] {
    let mut s = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut s);
    s
}

/// Derive a 32-byte key from a passphrase + salt using Argon2id.
pub fn derive_key(passphrase: &str, salt: &[u8]) -> anyhow::Result<[u8; KEY_LEN]> {
    if passphrase.is_empty() {
        anyhow::bail!("encryption passphrase cannot be empty");
    }
    let params = argon2::Params::new(
        ARGON2_M_COST,
        ARGON2_T_COST,
        ARGON2_P_COST,
        Some(KEY_LEN),
    )
    .map_err(|e| anyhow::anyhow!("invalid Argon2 params: {e}"))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; KEY_LEN];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| anyhow::anyhow!("Argon2 derivation failed: {e}"))?;
    Ok(out)
}

fn random_nonce_prefix() -> [u8; NONCE_PREFIX_LEN] {
    let mut p = [0u8; NONCE_PREFIX_LEN];
    rand::thread_rng().fill_bytes(&mut p);
    p
}

/// Per-chunk 12-byte nonce: 7-byte file prefix || 4-byte BE counter ||
/// 1-byte last-chunk flag.
fn build_nonce(prefix: &[u8; NONCE_PREFIX_LEN], idx: u32, is_last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..NONCE_PREFIX_LEN].copy_from_slice(prefix);
    n[NONCE_PREFIX_LEN..NONCE_PREFIX_LEN + 4].copy_from_slice(&idx.to_be_bytes());
    n[11] = if is_last { 0x01 } else { 0x00 };
    n
}

/// Bind chunk index and last-flag into the AEAD's associated data so that
/// reordering or splicing chunks fails decryption even if an attacker
/// could otherwise reproduce a valid nonce.
fn chunk_aad(idx: u32, is_last: bool) -> [u8; 5] {
    let mut a = [0u8; 5];
    a[..4].copy_from_slice(&idx.to_be_bytes());
    a[4] = if is_last { 0x01 } else { 0x00 };
    a
}

/// Like `read_exact` but returns the actual byte count read on EOF
/// instead of erroring. Used so the encrypt/decrypt loops can detect
/// short final chunks naturally.
async fn read_full<R: AsyncReadExt + Unpin>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_cipher() -> FileCipher {
        FileCipher::from_key([0x42; KEY_LEN])
    }

    async fn roundtrip(bytes: &[u8]) {
        let dir = TempDir::new().unwrap();
        let plain = dir.path().join("in");
        let cipher_path = dir.path().join("ct");
        let out = dir.path().join("out");
        tokio::fs::write(&plain, bytes).await.unwrap();

        let c = make_cipher();
        c.encrypt_file(&plain, &cipher_path).await.unwrap();
        c.decrypt_file(&cipher_path, &out).await.unwrap();

        let recovered = tokio::fs::read(&out).await.unwrap();
        assert_eq!(recovered, bytes);
    }

    #[tokio::test]
    async fn roundtrip_empty() {
        roundtrip(b"").await;
    }

    #[tokio::test]
    async fn roundtrip_small() {
        roundtrip(b"hello, world!").await;
    }

    #[tokio::test]
    async fn roundtrip_one_chunk_exactly() {
        let bytes = vec![0xABu8; PLAINTEXT_CHUNK];
        roundtrip(&bytes).await;
    }

    #[tokio::test]
    async fn roundtrip_multi_chunk() {
        // 3 full chunks + a short tail
        let bytes = vec![0x55u8; PLAINTEXT_CHUNK * 3 + 1234];
        roundtrip(&bytes).await;
    }

    #[tokio::test]
    async fn wrong_key_fails() {
        let dir = TempDir::new().unwrap();
        let plain = dir.path().join("in");
        let ct = dir.path().join("ct");
        let out = dir.path().join("out");
        tokio::fs::write(&plain, b"secret").await.unwrap();

        let a = FileCipher::from_key([1u8; KEY_LEN]);
        let b = FileCipher::from_key([2u8; KEY_LEN]);
        a.encrypt_file(&plain, &ct).await.unwrap();
        let err = b.decrypt_file(&ct, &out).await.unwrap_err();
        assert!(err.to_string().contains("AEAD failure"));
    }

    #[tokio::test]
    async fn ciphertext_differs_from_plaintext() {
        let dir = TempDir::new().unwrap();
        let plain = dir.path().join("in");
        let ct = dir.path().join("ct");
        tokio::fs::write(&plain, b"the quick brown fox").await.unwrap();
        make_cipher().encrypt_file(&plain, &ct).await.unwrap();
        let bytes = tokio::fs::read(&ct).await.unwrap();
        assert!(bytes.starts_with(MAGIC));
        assert!(!bytes.windows(3).any(|w| w == b"fox"));
    }

    #[tokio::test]
    async fn truncation_fails() {
        let dir = TempDir::new().unwrap();
        let plain = dir.path().join("in");
        let ct = dir.path().join("ct");
        let out = dir.path().join("out");
        tokio::fs::write(&plain, vec![1u8; PLAINTEXT_CHUNK * 2 + 100])
            .await
            .unwrap();
        let c = make_cipher();
        c.encrypt_file(&plain, &ct).await.unwrap();

        // Lop off the final chunk: the previous chunk now bears the
        // last-flag according to size math, but its nonce was generated
        // with last=0 → AEAD must fail.
        let mut bytes = tokio::fs::read(&ct).await.unwrap();
        bytes.truncate(HEADER_LEN + CIPHERTEXT_CHUNK);
        tokio::fs::write(&ct, &bytes).await.unwrap();
        assert!(c.decrypt_file(&ct, &out).await.is_err());
    }

    #[test]
    fn key_verifier_roundtrip() {
        let c = make_cipher();
        let v = c.make_verifier(b"pair-id-123").unwrap();
        assert!(c.verify(b"pair-id-123", &v).unwrap());
        assert!(!c.verify(b"wrong", &v).unwrap());

        let other = FileCipher::from_key([0x11; KEY_LEN]);
        assert!(!other.verify(b"pair-id-123", &v).unwrap());
    }

    #[test]
    fn passphrase_derivation_is_deterministic() {
        let salt = [0xCDu8; SALT_LEN];
        let k1 = derive_key("hunter2", &salt).unwrap();
        let k2 = derive_key("hunter2", &salt).unwrap();
        assert_eq!(k1, k2);
        let k3 = derive_key("hunter3", &salt).unwrap();
        assert_ne!(k1, k3);
    }
}

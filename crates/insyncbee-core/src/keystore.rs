//! Thin wrapper over the OS keyring used to persist per-sync-pair
//! encryption keys.
//!
//! The 32-byte derived key is stored as a hex string under
//! `service = "insyncbee", account = "pair:<sync_pair_id>"`. We don't put
//! the raw passphrase anywhere — only the derived key, so re-running the
//! KDF isn't needed every sync.

use crate::crypto::{FileCipher, KEY_LEN};

const SERVICE: &str = "insyncbee";

/// Construct the keyring entry account name for a given sync pair.
fn account_for(pair_id: &str) -> String {
    format!("pair:{pair_id}")
}

fn entry_for(pair_id: &str) -> anyhow::Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, &account_for(pair_id))
        .map_err(|e| anyhow::anyhow!("keyring: open entry for {pair_id}: {e}"))
}

/// Persist the derived key for `pair_id` in the OS keyring.
pub fn store_key(pair_id: &str, cipher: &FileCipher) -> anyhow::Result<()> {
    let hex = bytes_to_hex(cipher.key_bytes());
    entry_for(pair_id)?
        .set_password(&hex)
        .map_err(|e| anyhow::anyhow!("keyring: set key for {pair_id}: {e}"))?;
    Ok(())
}

/// Try to load the derived key for `pair_id` from the OS keyring.
/// Returns `Ok(None)` if no entry exists yet (so the UI can prompt the
/// user). Any other failure is surfaced as an error.
pub fn load_cipher(pair_id: &str) -> anyhow::Result<Option<FileCipher>> {
    let entry = entry_for(pair_id)?;
    match entry.get_password() {
        Ok(hex) => {
            let bytes = hex_to_bytes(&hex)?;
            if bytes.len() != KEY_LEN {
                anyhow::bail!(
                    "keyring: stored key for {pair_id} is {} bytes, expected {KEY_LEN}",
                    bytes.len()
                );
            }
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            Ok(Some(FileCipher::from_key(key)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("keyring: read key for {pair_id}: {e}")),
    }
}

/// Forget the derived key for `pair_id`. Called when a sync pair is
/// deleted or encryption is disabled. Missing entries are not an error.
pub fn delete_key(pair_id: &str) -> anyhow::Result<()> {
    match entry_for(pair_id)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("keyring: delete key for {pair_id}: {e}")),
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

fn hex_to_bytes(s: &str) -> anyhow::Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        anyhow::bail!("hex string has odd length");
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in s.as_bytes().chunks_exact(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(c: u8) -> anyhow::Result<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => anyhow::bail!("invalid hex character: {c:#x}"),
    }
}

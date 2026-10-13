//! Sealing a snapshot into a slot's blob, and opening one again.
//!
//! JSON, gzipped, then XChaCha20-Poly1305 under the code's `enc` key with a
//! random 24-byte nonce -- big enough that a random one never repeats, which is
//! the only thing that construction asks of its caller. The machine id is the
//! associated data, so a slot's ciphertext cannot be replayed into another
//! machine's slot by the server: it would no longer authenticate.
//!
//! Wire form: `0x01 ‖ nonce ‖ ciphertext+tag`, base64url without padding.

use std::io::{Read, Write};

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::code::random;
use super::snapshot::{Snapshot, SNAPSHOT_FORMAT};

/// The blob's first byte. A different one is a construction this build does
/// not know, which `open` treats like any other blob it cannot read.
pub const BLOB_VERSION: u8 = 1;

/// 512 KiB of raw blob, as base64url: what the server accepts, so checked here
/// before a request is wasted on it.
pub const MAX_BLOB_CHARS: usize = 699_051;

/// A slot that gunzips past this is refused rather than allowed to eat memory.
/// A real snapshot compresses ~10:1 and the blob is capped at 512 KiB, so an
/// honest one never comes near it.
const MAX_PLAINTEXT: u64 = 32 * 1024 * 1024;

const NONCE_LEN: usize = 24;

pub const TOO_LARGE: &str = "Your synced library is too large to sync.";

/// Why a slot could not be read. The two cases are handled differently by the
/// clock: an unreadable slot is skipped for good, a newer one is retried after
/// this machine has been updated.
#[derive(Debug)]
pub enum OpenError {
    /// Not base64, the wrong version byte, a failed tag, not gzip, not JSON.
    Unreadable(anyhow::Error),
    /// Decrypted fine, but written by a MyTube whose snapshot format this build
    /// does not understand. The machine name is kept when it can be read, so
    /// the other computer can still be named in Settings.
    Newer { format: u32, machine_name: Option<String> },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(e) => write!(f, "unreadable slot: {e:#}"),
            Self::Newer { format, .. } => write!(f, "slot in snapshot format {format}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// A snapshot serialised once, with the hash the clock compares against
/// `pushed_hash`. Sealing and hashing the very same bytes is what makes "the
/// hash of what was pushed" mean exactly that.
pub struct Encoded {
    pub json: Vec<u8>,
    pub hash: String,
}

pub fn encode(snapshot: &Snapshot) -> Result<Encoded> {
    let json = serde_json::to_vec(snapshot)?;
    let hash = hex(&Sha256::digest(&json));
    Ok(Encoded { json, hash })
}

/// Hex SHA-256 of the snapshot's JSON. The snapshot lists are ordered by id, so
/// an unchanged library always hashes the same.
pub fn snapshot_hash(snapshot: &Snapshot) -> Result<String> {
    Ok(encode(snapshot)?.hash)
}

/// Seals already-encoded snapshot JSON for `machine_id`'s slot.
pub fn seal(json: &[u8], enc: &[u8; 32], machine_id: &str) -> Result<String> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(json)?;
    let packed = gz.finish()?;

    let cipher = XChaCha20Poly1305::new(&(*enc).into());
    let nonce: [u8; NONCE_LEN] = random()?;
    let sealed = cipher
        .encrypt(&XNonce::from(nonce), Payload { msg: &packed, aad: machine_id.as_bytes() })
        .map_err(|_| anyhow!("could not encrypt the sync snapshot"))?;

    let mut raw = Vec::with_capacity(1 + NONCE_LEN + sealed.len());
    raw.push(BLOB_VERSION);
    raw.extend_from_slice(&nonce);
    raw.extend_from_slice(&sealed);
    let blob = URL_SAFE_NO_PAD.encode(raw);
    if blob.len() > MAX_BLOB_CHARS {
        anyhow::bail!(TOO_LARGE);
    }
    Ok(blob)
}

/// The snapshot's own `format`, read before the rest so a newer shape can be
/// told apart from a broken one.
#[derive(Deserialize)]
struct Header {
    format: u32,
    #[serde(default)]
    machine_name: Option<String>,
}

/// Opens a slot written by `machine_id`.
pub fn open(blob: &str, enc: &[u8; 32], machine_id: &str) -> Result<Snapshot, OpenError> {
    let json = unseal(blob, enc, machine_id).map_err(OpenError::Unreadable)?;
    let header: Header = serde_json::from_slice(&json)
        .context("the slot's snapshot is not JSON")
        .map_err(OpenError::Unreadable)?;
    if header.format > SNAPSHOT_FORMAT {
        return Err(OpenError::Newer { format: header.format, machine_name: header.machine_name });
    }
    serde_json::from_slice(&json)
        .context("the slot's snapshot is not readable")
        .map_err(OpenError::Unreadable)
}

fn unseal(blob: &str, enc: &[u8; 32], machine_id: &str) -> Result<Vec<u8>> {
    let raw = URL_SAFE_NO_PAD.decode(blob).context("the slot is not base64url")?;
    let Some((&version, rest)) = raw.split_first() else {
        anyhow::bail!("the slot is empty");
    };
    if version != BLOB_VERSION {
        anyhow::bail!("the slot is blob version {version}");
    }
    if rest.len() < NONCE_LEN {
        anyhow::bail!("the slot is truncated");
    }
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let nonce = XNonce::try_from(nonce).map_err(|_| anyhow!("the slot's nonce is malformed"))?;
    let cipher = XChaCha20Poly1305::new(&(*enc).into());
    let packed = cipher
        .decrypt(&nonce, Payload { msg: sealed, aad: machine_id.as_bytes() })
        // A failed tag says nothing about why -- another code, another slot's
        // ciphertext, a flipped bit -- and there is nothing more to say.
        .map_err(|_| anyhow!("the slot does not decrypt with this code"))?;

    let mut json = Vec::new();
    flate2::read::GzDecoder::new(packed.as_slice())
        .take(MAX_PLAINTEXT + 1)
        .read_to_end(&mut json)
        .context("the slot is not gzip")?;
    if json.len() as u64 > MAX_PLAINTEXT {
        anyhow::bail!("the slot unpacks past {MAX_PLAINTEXT} bytes");
    }
    Ok(json)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::snapshot::{SyncChannel, SyncVideo, Tombstone, TombstoneKind};

    const KEY: [u8; 32] = [7; 32];
    const ME: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn snapshot() -> Snapshot {
        Snapshot {
            format: SNAPSHOT_FORMAT,
            machine_name: "desk".into(),
            channels: vec![SyncChannel {
                id: "UC1".into(),
                title: "One".into(),
                handle: Some("@one".into()),
                url: "https://www.youtube.com/channel/UC1".into(),
                subscribed: true,
                subscribed_changed_at: Some(100),
                member: false,
                member_changed_at: None,
            }],
            videos: vec![SyncVideo {
                id: "vid00000001".into(),
                channel_id: "UC1".into(),
                title: "Part 1".into(),
                published_at: Some(50),
                sort_at: Some(50),
                duration_secs: Some(600),
                added_manually: false,
                added_at: 60,
                watched: true,
                watched_at: Some(70),
                watched_changed_at: Some(70),
                hidden: false,
                hidden_changed_at: None,
                sibling_group: None,
                sibling_changed_at: None,
                series_hidden: true,
                series_hidden_changed_at: Some(80),
            }],
            tombstones: vec![Tombstone { kind: TombstoneKind::Video, id: "gone".into(), removed_at: 9 }],
        }
    }

    fn sealed() -> String {
        seal(&encode(&snapshot()).unwrap().json, &KEY, ME).unwrap()
    }

    #[test]
    fn a_sealed_snapshot_opens_to_itself() {
        let blob = sealed();
        assert!(blob.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_eq!(open(&blob, &KEY, ME).unwrap(), snapshot());
    }

    #[test]
    fn the_blob_is_version_nonce_ciphertext() {
        let raw = URL_SAFE_NO_PAD.decode(sealed()).unwrap();
        assert_eq!(raw[0], BLOB_VERSION);
        // Version, nonce, the gzip stream, and Poly1305's 16-byte tag.
        assert!(raw.len() > 1 + NONCE_LEN + 16);
    }

    #[test]
    fn every_seal_takes_a_fresh_nonce() {
        let (a, b) = (sealed(), sealed());
        assert_ne!(a, b);
        assert_eq!(open(&a, &KEY, ME).unwrap(), open(&b, &KEY, ME).unwrap());
    }

    #[test]
    fn a_flipped_bit_anywhere_is_refused() {
        let raw = URL_SAFE_NO_PAD.decode(sealed()).unwrap();
        for at in [1, NONCE_LEN + 1, raw.len() / 2, raw.len() - 1] {
            let mut bad = raw.clone();
            bad[at] ^= 0x01;
            let err = open(&URL_SAFE_NO_PAD.encode(&bad), &KEY, ME).unwrap_err();
            assert!(matches!(err, OpenError::Unreadable(_)), "byte {at}: {err}");
        }
    }

    #[test]
    fn another_machines_id_as_associated_data_is_refused() {
        // The server moving one machine's ciphertext into another's slot.
        let err = open(&sealed(), &KEY, "BBBBBBBBBBBBBBBBBBBBBB").unwrap_err();
        assert!(matches!(err, OpenError::Unreadable(_)), "{err}");
    }

    #[test]
    fn another_codes_key_is_refused() {
        let err = open(&sealed(), &[8; 32], ME).unwrap_err();
        assert!(err.to_string().contains("does not decrypt"), "{err}");
    }

    #[test]
    fn garbage_truncation_and_an_unknown_version_are_refused() {
        assert!(matches!(open("not base64!", &KEY, ME), Err(OpenError::Unreadable(_))));
        assert!(matches!(open("", &KEY, ME), Err(OpenError::Unreadable(_))));
        assert!(matches!(open("AQID", &KEY, ME), Err(OpenError::Unreadable(_))));
        let mut raw = URL_SAFE_NO_PAD.decode(sealed()).unwrap();
        raw[0] = 2;
        let err = open(&URL_SAFE_NO_PAD.encode(&raw), &KEY, ME).unwrap_err();
        assert!(err.to_string().contains("blob version 2"), "{err}");
    }

    #[test]
    fn a_newer_format_is_its_own_error_and_keeps_the_machine_name() {
        // A future snapshot need not parse as this build's Snapshot at all.
        let json = br#"{"format":2,"machine_name":"laptop","everything":"else"}"#;
        let blob = seal(json, &KEY, ME).unwrap();
        match open(&blob, &KEY, ME).unwrap_err() {
            OpenError::Newer { format, machine_name } => {
                assert_eq!(format, 2);
                assert_eq!(machine_name.as_deref(), Some("laptop"));
            }
            other => panic!("expected Newer, got {other}"),
        }
    }

    #[test]
    fn a_current_format_with_a_broken_body_is_unreadable_not_newer() {
        let blob = seal(br#"{"format":1,"machine_name":"x"}"#, &KEY, ME).unwrap();
        assert!(matches!(open(&blob, &KEY, ME), Err(OpenError::Unreadable(_))));
    }

    #[test]
    fn a_gzip_bomb_is_refused_at_32_mib() {
        // 33 MiB of zeros gzips to a few dozen KiB: well inside the blob cap,
        // far outside what an honest snapshot unpacks to.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        let zeros = vec![0u8; 1024 * 1024];
        for _ in 0..33 {
            gz.write_all(&zeros).unwrap();
        }
        let packed = gz.finish().unwrap();
        let cipher = XChaCha20Poly1305::new(&KEY.into());
        let nonce = [3u8; NONCE_LEN];
        let sealed = cipher
            .encrypt(&XNonce::from(nonce), Payload { msg: &packed, aad: ME.as_bytes() })
            .unwrap();
        let mut raw = vec![BLOB_VERSION];
        raw.extend_from_slice(&nonce);
        raw.extend_from_slice(&sealed);
        let err = open(&URL_SAFE_NO_PAD.encode(raw), &KEY, ME).unwrap_err();
        assert!(err.to_string().contains("unpacks past"), "{err}");
    }

    #[test]
    fn sealing_refuses_a_blob_past_the_servers_limit() {
        // Random bytes do not compress, so 600 KiB of them seals past 512 KiB.
        let mut noise = vec![0u8; 600 * 1024];
        getrandom::fill(&mut noise).unwrap();
        let json = serde_json::to_vec(&URL_SAFE_NO_PAD.encode(&noise)).unwrap();
        let err = seal(&json, &KEY, ME).unwrap_err();
        assert_eq!(err.to_string(), TOO_LARGE);
    }

    #[test]
    fn the_hash_is_hex_sha256_of_the_json_and_follows_the_content() {
        let s = snapshot();
        let h = snapshot_hash(&s).unwrap();
        assert_eq!(h.len(), 64);
        assert_eq!(h, hex(&Sha256::digest(serde_json::to_vec(&s).unwrap())));
        assert_eq!(h, snapshot_hash(&s.clone()).unwrap(), "the same library hashes the same");
        let mut changed = s;
        changed.videos[0].watched = false;
        assert_ne!(h, snapshot_hash(&changed).unwrap());
    }
}

//! The sync code, and the two keys and the machine id that hang off it.
//!
//! The code *is* the group: 128 random bits, written so a person can copy it
//! from one screen to another. Everything else is derived from it -- the bearer
//! the server knows the group by, and the key that seals every slot -- so there
//! is no account, no email, and nothing on the server that could decrypt a
//! slot or even write to the group without the code itself.
//!
//! There is deliberately no checksum: a mistyped code derives a bearer for a
//! group that does not exist, and the server's `404 no_group` already says so.

use anyhow::{bail, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hkdf::Hkdf;
use sha2::Sha256;

/// The version prefix. A future code format would take `MYT2`, and this build
/// would refuse it by name rather than derive keys for a group it cannot read.
pub const PREFIX: &str = "MYT1";

/// Crockford's base32: no I, L, O or U, so the letters a person is likely to
/// misread are simply not there to be misread.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// 26 five-bit digits hold 130 bits, so the first carries only the top 3 of
/// the 128 and can be at most 7.
const DIGITS: usize = 26;

/// HKDF's salt. Bumping it would orphan every existing group, so it never moves
/// within `MYT1`.
const SALT: &[u8] = b"mytube-sync-v1";

/// The 16 secret bytes behind a sync code.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SyncCode([u8; 16]);

/// The code is the whole secret: a log line or a panic message printing one
/// would hand the group to whoever reads it.
impl std::fmt::Debug for SyncCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SyncCode(..)")
    }
}

/// What a code derives: both halves are 32 bytes of HKDF-SHA256 output.
pub struct Keys {
    /// `Authorization: Bearer` -- 43 characters of base64url. The server keys
    /// the group by a hash of this, so it holds nothing that could write to it.
    pub bearer: String,
    /// Seals and opens every slot. Never leaves this machine.
    pub enc: [u8; 32],
}

impl SyncCode {
    /// A fresh code from the OS's random source.
    pub fn generate() -> Result<Self> {
        Ok(Self(random()?))
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Reads a code however it was copied: any case, with or without its
    /// hyphens, with spaces from a line wrap, and with `O` for `0` and `I` or
    /// `L` for `1` -- the misreadings Crockford's alphabet exists to absorb.
    pub fn parse(input: &str) -> Result<Self> {
        let cleaned: String = input
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .map(|c| match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            })
            .collect();
        let Some(digits) = cleaned.strip_prefix(PREFIX) else {
            bail!("That is not a MyTube sync code: it should start with {PREFIX}.");
        };
        if digits.chars().count() != DIGITS {
            bail!("That sync code is the wrong length. Check it and try again.");
        }
        let mut value: u128 = 0;
        for (i, c) in digits.chars().enumerate() {
            let Some(d) = ALPHABET.iter().position(|&a| a as char == c) else {
                bail!("A sync code cannot contain \u{201c}{c}\u{201d}. Check it and try again.");
            };
            // The first digit carries 3 bits, not 5; anything above 7 there is
            // a value no generated code can have.
            if i == 0 && d > 7 {
                bail!("That is not a valid sync code. Check it and try again.");
            }
            value = (value << 5) | d as u128;
        }
        Ok(Self(value.to_be_bytes()))
    }

    /// HKDF-SHA256 over the code's 16 bytes. Pinned by a fixed-vector test: a
    /// change here would silently split every existing group in two.
    pub fn keys(&self) -> Keys {
        let hk = Hkdf::<Sha256>::new(Some(SALT), &self.0);
        let mut auth = [0u8; 32];
        let mut enc = [0u8; 32];
        // 32 bytes is far inside HKDF-SHA256's 8160-byte limit, so neither
        // expand can fail.
        hk.expand(b"auth", &mut auth).expect("32 bytes is a valid HKDF length");
        hk.expand(b"enc", &mut enc).expect("32 bytes is a valid HKDF length");
        Keys { bearer: URL_SAFE_NO_PAD.encode(auth), enc }
    }
}

/// `MYT1-XXXXX-XXXXX-XXXXX-XXXXX-XXXXXX`: most significant digit first,
/// grouped 5-5-5-5-6 so it can be read aloud and compared at a glance.
impl std::fmt::Display for SyncCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = u128::from_be_bytes(self.0);
        let digits: Vec<char> = (0..DIGITS)
            .map(|i| ALPHABET[((v >> (5 * (DIGITS - 1 - i))) & 31) as usize] as char)
            .collect();
        f.write_str(PREFIX)?;
        let mut at = 0;
        for len in [5, 5, 5, 5, 6] {
            f.write_str("-")?;
            f.write_str(&digits[at..at + len].iter().collect::<String>())?;
            at += len;
        }
        Ok(())
    }
}

/// A fresh machine id: 16 random bytes, 22 characters of base64url. Made on
/// every Turn on and Join, so leaving and rejoining takes a new slot rather
/// than inheriting the old one.
pub fn new_machine_id() -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(random::<16>()?))
}

/// `N` bytes from the OS's random source.
pub(crate) fn random<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|e| anyhow::anyhow!("no random source: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COUNTING: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

    #[test]
    fn a_code_is_written_most_significant_digit_first_in_five_groups() {
        // Vectors computed outside Rust (Python's int and a hand-written
        // alphabet), so this pins the format rather than restating the code.
        assert_eq!(
            SyncCode::from_bytes(COUNTING).to_string(),
            "MYT1-00041-06105-0R3GG-28A1C-60T3GF"
        );
        assert_eq!(SyncCode::from_bytes([0; 16]).to_string(), "MYT1-00000-00000-00000-00000-000000");
        assert_eq!(
            SyncCode::from_bytes([0xff; 16]).to_string(),
            "MYT1-7ZZZZ-ZZZZZ-ZZZZZ-ZZZZZ-ZZZZZZ"
        );
    }

    #[test]
    fn a_generated_code_round_trips_through_its_text() {
        for _ in 0..200 {
            let code = SyncCode::generate().unwrap();
            let text = code.to_string();
            assert_eq!(text.len(), 4 + 5 * 5 + 6, "{text}");
            assert!(text.chars().nth(5).unwrap() <= '7', "{text}");
            assert_eq!(SyncCode::parse(&text).unwrap(), code);
        }
    }

    #[test]
    fn two_generated_codes_differ() {
        assert_ne!(SyncCode::generate().unwrap(), SyncCode::generate().unwrap());
    }

    #[test]
    fn parsing_forgives_case_spacing_missing_hyphens_and_misread_letters() {
        let code = SyncCode::from_bytes(COUNTING);
        for pasted in [
            "MYT1-00041-06105-0R3GG-28A1C-60T3GF",
            "myt1-00041-06105-0r3gg-28a1c-60t3gf",
            "MYT10004106105 0R3GG28A1C60T3GF",
            "  MYT1 00041 06105\n0R3GG 28A1C 60T3GF \t",
            "MYT1-OOO41-O61O5-OR3GG-28AIC-6OT3GF",
            "MYT1-ooo4l-o6lo5-or3gg-28aLc-6ot3gf",
            // The prefix's own 1 misread as I is the same code.
            "MYTI-00041-06105-0R3GG-28A1C-60T3GF",
        ] {
            assert_eq!(SyncCode::parse(pasted).unwrap(), code, "{pasted:?}");
        }
    }

    #[test]
    fn parsing_refuses_a_wrong_prefix() {
        for bad in ["", "MYT2-00041-06105-0R3GG-28A1C-60T3GF", "00041-06105-0R3GG-28A1C-60T3GF"] {
            let err = SyncCode::parse(bad).unwrap_err().to_string();
            assert!(err.contains("start with MYT1"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn parsing_refuses_a_code_one_digit_short_or_long() {
        for bad in ["MYT1-00041-06105-0R3GG-28A1C-60T3G", "MYT1-00041-06105-0R3GG-28A1C-60T3GF0"] {
            let err = SyncCode::parse(bad).unwrap_err().to_string();
            assert!(err.contains("wrong length"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn parsing_refuses_characters_outside_the_alphabet() {
        // U is the one letter Crockford leaves out without reading it as a digit.
        let err = SyncCode::parse("MYT1-00041-06105-0R3GG-28A1C-60T3GU").unwrap_err().to_string();
        assert!(err.contains('U'), "{err}");
        assert!(SyncCode::parse("MYT1-00041-06105-0R3GG-28A1C-60T3G!").is_err());
    }

    #[test]
    fn parsing_refuses_a_value_past_128_bits() {
        assert!(SyncCode::parse("MYT1-7ZZZZ-ZZZZZ-ZZZZZ-ZZZZZ-ZZZZZZ").is_ok());
        let err = SyncCode::parse("MYT1-8ZZZZ-ZZZZZ-ZZZZZ-ZZZZZ-ZZZZZZ").unwrap_err().to_string();
        assert!(err.contains("not a valid sync code"), "{err}");
        assert!(SyncCode::parse("MYT1-Z0000-00000-00000-00000-000000").is_err());
    }

    #[test]
    fn key_derivation_is_pinned_to_fixed_vectors() {
        // Computed with Python's hmac/hashlib (RFC 5869 by hand), not with this
        // crate: if either side drifts, every existing group splits in two.
        let k = SyncCode::from_bytes(COUNTING).keys();
        assert_eq!(k.bearer, "Llmm4-FtqobVZwturlPDbjcnyu1fxNo2db_fNmeYVAs");
        assert_eq!(
            hex(&k.enc),
            "f8a0758bc17cc1d3a5ab470b0fa681e709d44e31b2bcc0cb100ecfb0dba696cd"
        );
        let k = SyncCode::from_bytes([0xff; 16]).keys();
        assert_eq!(k.bearer, "a-n6A97BjzZEm2JF7-KH-APOHa7wIFHb2W6E5csXF58");
        assert_eq!(
            hex(&k.enc),
            "e530a71acfe0aa6e6294fa73aaefb67b4ff7a508abd4c57cf15fe24bb7c2e790"
        );
    }

    #[test]
    fn key_derivation_is_deterministic_and_the_two_keys_differ() {
        let code = SyncCode::generate().unwrap();
        let (a, b) = (code.keys(), SyncCode::parse(&code.to_string()).unwrap().keys());
        assert_eq!(a.bearer, b.bearer);
        assert_eq!(a.enc, b.enc);
        assert_eq!(a.bearer.len(), 43);
        assert!(a.bearer.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_ne!(URL_SAFE_NO_PAD.decode(&a.bearer).unwrap(), a.enc.to_vec());
    }

    #[test]
    fn a_machine_id_is_22_characters_of_base64url() {
        let id = new_machine_id().unwrap();
        assert_eq!(id.len(), 22, "{id}");
        assert!(id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'), "{id}");
        assert_ne!(id, new_machine_id().unwrap());
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let code = SyncCode::from_bytes(COUNTING);
        assert_eq!(format!("{code:?}"), "SyncCode(..)");
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}

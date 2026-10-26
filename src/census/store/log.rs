//! Byte layout of the version-2 account file.
//!
//! ```text
//! MAGIC_V2 | base length (u64 LE) | nonce | AES-256-GCM(SQLite snapshot)
//! frame*:    frame length (u32 LE) | nonce | AES-256-GCM(journal frame)
//! ```
//!
//! The base authenticates the magic and account. Each frame also
//! authenticates the base tag and its own sequence number, so frames cannot
//! move between files, between bases or within one log. A frame is appended
//! and synced before the save that wrote it returns. A failed append can only
//! leave bytes after the last committed frame, so a damaged region is
//! accepted as an uncommitted tail only when it reaches the end of the file or
//! holds nothing but zeros. Damage anywhere else is an authentication error.

use super::CensusError;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, Payload},
};
use rand::{RngCore, rngs::OsRng};
use zeroize::Zeroizing;

pub(super) const MAGIC_V2: &[u8] = b"MAILROOM-CENSUS\x02";
const NONCE: usize = 12;
const TAG: usize = 16;
const BASE_HEADER: usize = MAGIC_V2.len() + 8 + NONCE;
const FRAME_HEADER: usize = 4 + NONCE;
pub(super) const MAX_FRAME: usize = 64 * 1024 * 1024;

fn base_aad(account: &str) -> Vec<u8> {
    [MAGIC_V2, account.as_bytes()].concat()
}

fn frame_aad(account: &str, base_tag: &[u8; TAG], sequence: u64) -> Vec<u8> {
    [
        MAGIC_V2,
        account.as_bytes(),
        b"\0journal-frame\0",
        base_tag,
        &sequence.to_le_bytes(),
    ]
    .concat()
}

fn nonce() -> [u8; NONCE] {
    let mut nonce = [0; NONCE];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Encrypts a base snapshot and returns the file prefix and its tag.
pub(super) fn encode_base(
    cipher: &Aes256Gcm,
    account: &str,
    sqlite: &[u8],
) -> Result<(Vec<u8>, [u8; TAG]), CensusError> {
    let nonce = nonce();
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: sqlite,
                aad: &base_aad(account),
            },
        )
        .map_err(|_| CensusError::Storage)?;
    let tag = encrypted[encrypted.len() - TAG..].try_into().unwrap();
    let mut bytes = Vec::with_capacity(BASE_HEADER + encrypted.len());
    bytes.extend_from_slice(MAGIC_V2);
    bytes.extend_from_slice(&(encrypted.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&encrypted);
    Ok((bytes, tag))
}

pub(super) fn encode_frame(
    cipher: &Aes256Gcm,
    account: &str,
    base_tag: &[u8; TAG],
    sequence: u64,
    plaintext: &[u8],
) -> Result<Vec<u8>, CensusError> {
    let nonce = nonce();
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &frame_aad(account, base_tag, sequence),
            },
        )
        .map_err(|_| CensusError::Storage)?;
    if encrypted.len() > MAX_FRAME {
        return Err(CensusError::Storage);
    }
    let mut bytes = Vec::with_capacity(FRAME_HEADER + encrypted.len());
    bytes.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&encrypted);
    Ok(bytes)
}

pub(super) struct Decoded {
    pub sqlite: Zeroizing<Vec<u8>>,
    pub base_tag: [u8; TAG],
    pub base_len: u64,
    pub frames: Vec<Zeroizing<Vec<u8>>>,
    /// End of the last committed frame. Bytes after it are an uncommitted tail.
    pub valid_end: u64,
}

pub(super) fn decode(
    cipher: &Aes256Gcm,
    account: &str,
    bytes: &[u8],
) -> Result<Decoded, CensusError> {
    if bytes.len() < BASE_HEADER + TAG {
        return Err(CensusError::Authentication);
    }
    let length = u64::from_le_bytes(
        bytes[MAGIC_V2.len()..MAGIC_V2.len() + 8]
            .try_into()
            .unwrap(),
    );
    let base_end = (BASE_HEADER as u64)
        .checked_add(length)
        .filter(|end| *end <= bytes.len() as u64 && length >= TAG as u64)
        .ok_or(CensusError::Authentication)? as usize;
    let encrypted = &bytes[BASE_HEADER..base_end];
    let sqlite = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&bytes[MAGIC_V2.len() + 8..BASE_HEADER]),
                Payload {
                    msg: encrypted,
                    aad: &base_aad(account),
                },
            )
            .map_err(|_| CensusError::Authentication)?,
    );
    let base_tag: [u8; TAG] = encrypted[encrypted.len() - TAG..].try_into().unwrap();
    let mut frames = Vec::new();
    let mut offset = base_end;
    while offset < bytes.len() {
        let rest = &bytes[offset..];
        let frame = (rest.len() >= FRAME_HEADER)
            .then(|| u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize)
            .filter(|len| *len >= TAG && FRAME_HEADER + len <= rest.len())
            .and_then(|len| {
                cipher
                    .decrypt(
                        Nonce::from_slice(&rest[4..FRAME_HEADER]),
                        Payload {
                            msg: &rest[FRAME_HEADER..FRAME_HEADER + len],
                            aad: &frame_aad(account, &base_tag, frames.len() as u64),
                        },
                    )
                    .ok()
                    .map(|plain| (FRAME_HEADER + len, plain))
            });
        match frame {
            Some((len, plain)) => {
                frames.push(Zeroizing::new(plain));
                offset += len;
            }
            None => {
                let declared = (rest.len() >= 4).then(|| {
                    FRAME_HEADER + u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize
                });
                let reaches_end = declared.is_none_or(|end| end >= rest.len());
                if !reaches_end && rest.iter().any(|b| *b != 0) {
                    return Err(CensusError::Authentication);
                }
                break;
            }
        }
    }
    Ok(Decoded {
        sqlite,
        base_tag,
        base_len: base_end as u64,
        frames,
        valid_end: offset as u64,
    })
}

//! Version-3 recovery bundles. A personal-scale account file is hundreds of
//! megabytes, so the bundle is written and read in chunks and never held in
//! memory whole.
//!
//! ```text
//! MAGIC | u32 header length | nonce | AES-256-GCM(header JSON)
//! per captured file, per chunk: u32 length | nonce | AES-256-GCM(chunk)
//! ```
//!
//! The key is the separately held 32-byte recovery secret. The header carries
//! a random stream identity. Each chunk authenticates that identity, its file
//! index, its chunk index and whether it ends its file, so chunks cannot be
//! reordered, dropped, moved between files or bundles, or cut at a chunk
//! boundary. A bundle's identity is the SHA-256 of all its bytes.

use super::super::SecretValue;
use super::super::lifecycle::boundary;
use crate::acquisition::AcquisitionError;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use rand::{RngCore, rngs::OsRng};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufReader, BufWriter, Read, Seek, Write},
    path::Path,
};
use zeroize::Zeroizing;

pub(super) const MAGIC: &[u8] = b"MAILROOM-RECOVERY\x03";
pub(super) const EXTENSION: &str = "bundle";
const CHUNK: usize = 1 << 20;
const MAX_HEADER: usize = 4 << 20;
const NONCE: usize = 12;
const TAG: usize = 16;

#[derive(Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Header<T> {
    version: u32,
    stream: String,
    body: T,
}

fn cipher(secret: &SecretValue) -> Result<Aes256Gcm, AcquisitionError> {
    if secret.bytes().len() != 32 {
        return Err(AcquisitionError::InvalidInput);
    }
    Aes256Gcm::new_from_slice(secret.bytes()).map_err(|_| AcquisitionError::InvalidInput)
}

fn header_aad() -> Vec<u8> {
    [MAGIC, b"\0header"].concat()
}

fn chunk_aad(stream: &[u8; 16], file: u32, index: u64, last: bool) -> Vec<u8> {
    [
        MAGIC,
        b"\0chunk\0",
        stream,
        &file.to_le_bytes(),
        &index.to_le_bytes(),
        &[u8::from(last)],
    ]
    .concat()
}

struct Hashing<W> {
    inner: W,
    hasher: Sha256,
}
impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Writes a bundle into a private staged file in `directory`. Returns the
/// staged file, unpublished, and the bundle's identity.
pub(super) struct Writer {
    out: Hashing<BufWriter<tempfile::NamedTempFile>>,
    cipher: Aes256Gcm,
    stream: [u8; 16],
    files: u32,
}

impl Writer {
    pub(super) fn create(
        directory: &Path,
        prefix: &str,
        secret: &SecretValue,
        body: &impl Serialize,
    ) -> Result<Self, AcquisitionError> {
        let cipher = cipher(secret)?;
        let mut stream = [0; 16];
        OsRng.fill_bytes(&mut stream);
        let plain = Zeroizing::new(
            serde_json::to_vec(&Header {
                version: 3,
                stream: data_encoding::HEXLOWER.encode(&stream),
                body,
            })
            .map_err(|_| AcquisitionError::Storage)?,
        );
        if plain.len() > MAX_HEADER {
            return Err(AcquisitionError::InvalidInput);
        }
        let file = tempfile::Builder::new()
            .prefix(prefix)
            .tempfile_in(directory)
            .map_err(|_| AcquisitionError::Storage)?;
        let mut writer = Self {
            out: Hashing {
                inner: BufWriter::with_capacity(CHUNK, file),
                hasher: Sha256::new(),
            },
            cipher,
            stream,
            files: 0,
        };
        writer
            .out
            .write_all(MAGIC)
            .map_err(|_| AcquisitionError::Storage)?;
        writer.sealed(&plain, &header_aad())?;
        Ok(writer)
    }

    fn sealed(&mut self, plain: &[u8], aad: &[u8]) -> Result<(), AcquisitionError> {
        let mut nonce = [0; NONCE];
        OsRng.fill_bytes(&mut nonce);
        let encrypted = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
            .map_err(|_| AcquisitionError::Storage)?;
        let len = u32::try_from(encrypted.len()).map_err(|_| AcquisitionError::Storage)?;
        self.out
            .write_all(&len.to_le_bytes())
            .and_then(|_| self.out.write_all(&nonce))
            .and_then(|_| self.out.write_all(&encrypted))
            .map_err(|_| AcquisitionError::Storage)
    }

    /// Appends one captured file and returns its length and SHA-256.
    pub(super) fn file(
        &mut self,
        source: &mut impl Read,
    ) -> Result<(u64, String), AcquisitionError> {
        let index = self.files;
        self.files += 1;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut current = Zeroizing::new(read_chunk(source)?);
        for chunk in 0u64.. {
            let next = if current.len() == CHUNK {
                Zeroizing::new(read_chunk(source)?)
            } else {
                Zeroizing::new(Vec::new())
            };
            let last = next.is_empty();
            hasher.update(&*current);
            total += current.len() as u64;
            self.sealed(&current, &chunk_aad(&self.stream, index, chunk, last))?;
            if last {
                break;
            }
            current = next;
        }
        Ok((total, data_encoding::HEXLOWER.encode(&hasher.finalize())))
    }

    /// Flushes and syncs the staged bundle.
    pub(super) fn finish(self) -> Result<(tempfile::NamedTempFile, String), AcquisitionError> {
        let Hashing { inner, hasher } = self.out;
        let file = inner.into_inner().map_err(|_| AcquisitionError::Storage)?;
        file.as_file()
            .sync_all()
            .map_err(|_| AcquisitionError::Storage)?;
        Ok((file, data_encoding::HEXLOWER.encode(&hasher.finalize())))
    }
}

fn read_chunk(source: &mut impl Read) -> Result<Vec<u8>, AcquisitionError> {
    let mut chunk = Vec::with_capacity(CHUNK);
    source
        .take(CHUNK as u64)
        .read_to_end(&mut chunk)
        .map_err(|_| AcquisitionError::Storage)?;
    Ok(chunk)
}

/// Publishes a staged bundle under its identity without replacing a file.
pub(super) fn publish(
    staged: tempfile::NamedTempFile,
    directory: &Path,
    id: &str,
    staged_boundary: &str,
) -> Result<(), AcquisitionError> {
    boundary(staged_boundary)?;
    staged
        .persist_noclobber(directory.join(format!("{id}.{EXTENSION}")))
        .map_err(|_| AcquisitionError::Storage)?;
    super::super::lifecycle::sync_dir(directory)
}

struct Hashed<R> {
    inner: R,
    hasher: Sha256,
    read: u64,
}
impl<R: Read> Read for Hashed<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

/// Reads a bundle in one pass: the header, then each file into a sink the
/// caller chooses, then the end of the file.
pub(super) struct Reader {
    input: Hashed<BufReader<fs::File>>,
    cipher: Aes256Gcm,
    stream: [u8; 16],
    files: u32,
    len: u64,
}

impl Reader {
    pub(super) fn is_stream(path: &Path) -> Result<bool, AcquisitionError> {
        Self::is_stream_file(&fs::File::open(path).map_err(|_| AcquisitionError::Storage)?)
    }

    pub(super) fn is_stream_file(file: &fs::File) -> Result<bool, AcquisitionError> {
        let mut magic = vec![0; MAGIC.len()];
        let mut file = file.try_clone().map_err(|_| AcquisitionError::Storage)?;
        file.rewind().map_err(|_| AcquisitionError::Storage)?;
        match file.read_exact(&mut magic) {
            Ok(()) => Ok(magic == MAGIC),
            Err(_) => Ok(false),
        }
    }

    pub(super) fn open<T: DeserializeOwned>(
        path: &Path,
        secret: &SecretValue,
        max_len: u64,
    ) -> Result<(Self, T), AcquisitionError> {
        let metadata = fs::symlink_metadata(path).map_err(|_| AcquisitionError::Storage)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max_len {
            return Err(AcquisitionError::InvalidInput);
        }
        let file = fs::File::open(path).map_err(|_| AcquisitionError::Storage)?;
        Self::open_file(&file, secret, max_len)
    }

    pub(super) fn open_file<T: DeserializeOwned>(
        file: &fs::File,
        secret: &SecretValue,
        max_len: u64,
    ) -> Result<(Self, T), AcquisitionError> {
        let cipher = cipher(secret)?;
        let metadata = file.metadata().map_err(|_| AcquisitionError::Storage)?;
        if !metadata.is_file() || metadata.len() > max_len {
            return Err(AcquisitionError::InvalidInput);
        }
        let mut file = file.try_clone().map_err(|_| AcquisitionError::Storage)?;
        file.rewind().map_err(|_| AcquisitionError::Storage)?;
        let mut reader = Self {
            input: Hashed {
                inner: BufReader::with_capacity(CHUNK, file),
                hasher: Sha256::new(),
                read: 0,
            },
            cipher,
            stream: [0; 16],
            files: 0,
            len: metadata.len(),
        };
        let mut magic = vec![0; MAGIC.len()];
        reader
            .input
            .read_exact(&mut magic)
            .map_err(|_| AcquisitionError::InvalidInput)?;
        if magic != MAGIC {
            return Err(AcquisitionError::InvalidInput);
        }
        let plain = reader.opened(MAX_HEADER, &header_aad())?;
        let header: Header<T> =
            serde_json::from_slice(&plain).map_err(|_| AcquisitionError::InvalidInput)?;
        let stream = data_encoding::HEXLOWER
            .decode(header.stream.as_bytes())
            .ok()
            .and_then(|s| <[u8; 16]>::try_from(s).ok())
            .filter(|_| header.version == 3)
            .ok_or(AcquisitionError::InvalidInput)?;
        reader.stream = stream;
        Ok((reader, header.body))
    }

    fn opened(&mut self, limit: usize, aad: &[u8]) -> Result<Zeroizing<Vec<u8>>, AcquisitionError> {
        let mut len = [0; 4];
        self.input
            .read_exact(&mut len)
            .map_err(|_| AcquisitionError::InvalidInput)?;
        let len = u32::from_le_bytes(len) as usize;
        if len < TAG || len > limit + TAG || self.input.read + (NONCE + len) as u64 > self.len {
            return Err(AcquisitionError::InvalidInput);
        }
        let mut nonce = [0; NONCE];
        let mut sealed = vec![0; len];
        self.input
            .read_exact(&mut nonce)
            .and_then(|_| self.input.read_exact(&mut sealed))
            .map_err(|_| AcquisitionError::InvalidInput)?;
        Ok(Zeroizing::new(
            self.cipher
                .decrypt(Nonce::from_slice(&nonce), Payload { msg: &sealed, aad })
                .map_err(|_| AcquisitionError::Authentication)?,
        ))
    }

    /// Decrypts the next captured file into `sink`, refusing more than
    /// `limit` bytes. Returns its length and SHA-256.
    pub(super) fn file(
        &mut self,
        sink: &mut impl Write,
        limit: u64,
    ) -> Result<(u64, String), AcquisitionError> {
        let index = self.files;
        self.files += 1;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        for chunk in 0u64.. {
            let mut len = [0; 4];
            let at = self.input.read;
            self.input
                .read_exact(&mut len)
                .map_err(|_| AcquisitionError::InvalidInput)?;
            let sealed = u32::from_le_bytes(len) as usize;
            if sealed < TAG || sealed > CHUNK + TAG || at + (4 + NONCE + sealed) as u64 > self.len {
                return Err(AcquisitionError::InvalidInput);
            }
            let mut nonce = [0; NONCE];
            let mut body = vec![0; sealed];
            self.input
                .read_exact(&mut nonce)
                .and_then(|_| self.input.read_exact(&mut body))
                .map_err(|_| AcquisitionError::InvalidInput)?;
            let open = |last| {
                self.cipher
                    .decrypt(
                        Nonce::from_slice(&nonce),
                        Payload {
                            msg: &body,
                            aad: &chunk_aad(&self.stream, index, chunk, last),
                        },
                    )
                    .ok()
            };
            // Only a full chunk can continue its file.
            let (plain, last) = if sealed - TAG == CHUNK {
                open(false)
                    .map(|p| (p, false))
                    .or_else(|| open(true).map(|p| (p, true)))
            } else {
                open(true).map(|p| (p, true))
            }
            .ok_or(AcquisitionError::Authentication)?;
            let plain = Zeroizing::new(plain);
            total += plain.len() as u64;
            if total > limit {
                return Err(AcquisitionError::InvalidInput);
            }
            hasher.update(&*plain);
            sink.write_all(&plain)
                .map_err(|_| AcquisitionError::Storage)?;
            if last {
                break;
            }
        }
        Ok((total, data_encoding::HEXLOWER.encode(&hasher.finalize())))
    }

    /// Requires the end of the bundle and returns the bundle's identity.
    pub(super) fn finish(mut self) -> Result<String, AcquisitionError> {
        let mut rest = [0; 1];
        if self
            .input
            .read(&mut rest)
            .map_err(|_| AcquisitionError::Storage)?
            != 0
            || self.input.read != self.len
        {
            return Err(AcquisitionError::InvalidInput);
        }
        Ok(data_encoding::HEXLOWER.encode(&self.input.hasher.finalize()))
    }
}

/// SHA-256 and length of a file, read in chunks.
pub(super) fn file_digest(path: &Path) -> Result<(u64, String), AcquisitionError> {
    let mut input = Hashed {
        inner: BufReader::with_capacity(
            CHUNK,
            fs::File::open(path).map_err(|_| AcquisitionError::Storage)?,
        ),
        hasher: Sha256::new(),
        read: 0,
    };
    std::io::copy(&mut input, &mut std::io::sink()).map_err(|_| AcquisitionError::Storage)?;
    Ok((
        input.read,
        data_encoding::HEXLOWER.encode(&input.hasher.finalize()),
    ))
}

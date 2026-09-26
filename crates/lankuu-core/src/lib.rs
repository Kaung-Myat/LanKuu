use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub mod network;

pub const MAGIC: &[u8; 8] = b"LANKUU01";
pub const DEFAULT_TRANSFER_PORT: u16 = 45_454;
pub const DISCOVERY_PORT: u16 = 45_455;
pub const DISCOVERY_QUERY: &[u8] = b"LANKUU_DISCOVER_V1";
pub const DISCOVERY_RESPONSE_PREFIX: &str = "LANKUU_HERE_V1";
pub const MAX_NAME_BYTES: usize = 1_024;
pub const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
pub const ACK_OK: &[u8; 2] = b"OK";
pub const ACK_ERROR: &[u8; 2] = b"ER";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadKind {
    Text = 1,
    File = 2,
}

impl TryFrom<u8> for PayloadKind {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Text),
            2 => Ok(Self::File),
            other => Err(ProtocolError::InvalidKind(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub kind: PayloadKind,
    pub name: String,
    pub payload_len: u64,
}

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    InvalidMagic,
    InvalidKind(u8),
    InvalidNameLength(usize),
    InvalidUtf8,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::InvalidMagic => write!(formatter, "not a LanKuu v1 message"),
            Self::InvalidKind(kind) => write!(formatter, "unsupported payload kind: {kind}"),
            Self::InvalidNameLength(length) => {
                write!(formatter, "invalid name length: {length} bytes")
            }
            Self::InvalidUtf8 => write!(formatter, "payload name is not valid UTF-8"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn write_header<W: Write>(writer: &mut W, header: &Header) -> Result<(), ProtocolError> {
    let name = header.name.as_bytes();
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.len() > u16::MAX as usize {
        return Err(ProtocolError::InvalidNameLength(name.len()));
    }

    writer.write_all(MAGIC)?;
    writer.write_all(&[header.kind as u8])?;
    writer.write_all(&(name.len() as u16).to_be_bytes())?;
    writer.write_all(&header.payload_len.to_be_bytes())?;
    writer.write_all(name)?;
    Ok(())
}

pub fn read_header<R: Read>(reader: &mut R) -> Result<Header, ProtocolError> {
    let mut magic = [0_u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(ProtocolError::InvalidMagic);
    }

    let mut kind = [0_u8; 1];
    reader.read_exact(&mut kind)?;
    let kind = PayloadKind::try_from(kind[0])?;

    let mut name_len = [0_u8; 2];
    reader.read_exact(&mut name_len)?;
    let name_len = u16::from_be_bytes(name_len) as usize;
    if name_len == 0 || name_len > MAX_NAME_BYTES {
        return Err(ProtocolError::InvalidNameLength(name_len));
    }

    let mut payload_len = [0_u8; 8];
    reader.read_exact(&mut payload_len)?;
    let payload_len = u64::from_be_bytes(payload_len);

    let mut name = vec![0_u8; name_len];
    reader.read_exact(&mut name)?;
    let name = String::from_utf8(name).map_err(|_| ProtocolError::InvalidUtf8)?;

    Ok(Header {
        kind,
        name,
        payload_len,
    })
}

pub fn copy_exact<R: Read, W: Write>(reader: &mut R, writer: &mut W, size: u64) -> io::Result<u64> {
    let mut limited = reader.take(size);
    let copied = io::copy(&mut limited, writer)?;
    if copied != size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("expected {size} bytes but received {copied}"),
        ));
    }
    Ok(copied)
}

pub fn safe_file_name(raw: &str) -> String {
    let final_component = Path::new(raw)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("received.bin");

    let cleaned: String = final_component
        .chars()
        .map(|character| match character {
            '/' | '\\' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect();

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "received.bin".to_owned()
    } else {
        cleaned
    }
}

pub fn unique_destination(directory: &Path, requested_name: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let safe_name = safe_file_name(requested_name);
    let initial = directory.join(&safe_name);
    if !initial.exists() {
        return Ok(initial);
    }

    let path = Path::new(&safe_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("received");
    let extension = path.extension().and_then(|value| value.to_str());

    for suffix in 1_u32.. {
        let candidate_name = match extension {
            Some(extension) => format!("{stem} ({suffix}).{extension}"),
            None => format!("{stem} ({suffix})"),
        };
        let candidate = directory.join(candidate_name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    unreachable!("the suffix space is practically unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn header_round_trip_preserves_values() {
        let expected = Header {
            kind: PayloadKind::File,
            name: "မြန်မာ.txt".to_owned(),
            payload_len: 4_294_967_296,
        };
        let mut bytes = Vec::new();

        write_header(&mut bytes, &expected).unwrap();
        let actual = read_header(&mut Cursor::new(bytes)).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn invalid_magic_is_rejected() {
        let bytes = [0_u8; 19];
        let error = read_header(&mut Cursor::new(bytes)).unwrap_err();
        assert!(matches!(error, ProtocolError::InvalidMagic));
    }

    #[test]
    fn path_components_are_removed_from_file_names() {
        assert_eq!(safe_file_name("../../secret.txt"), "secret.txt");
        assert_eq!(safe_file_name("..\\secret.txt"), ".._secret.txt");
        assert_eq!(safe_file_name(".."), "received.bin");
    }

    #[test]
    fn copy_exact_detects_a_truncated_payload() {
        let mut input = Cursor::new(b"abc");
        let mut output = Vec::new();
        let error = copy_exact(&mut input, &mut output, 4).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}

//! Save files and the state hash.
//!
//! File layout (all saves): `b"FRMC"` (4 bytes) · [`FileKind`] (1 byte) · format version (`u32`
//! little-endian) · postcard payload. The header is fixed-width so any future reader can sniff it.
//! Bump [`FORMAT_VERSION`] whenever serialized state changes shape, and add a migration or a clear
//! error for old files.

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::sim::{CommandLog, SimState};

pub const MAGIC: [u8; 4] = *b"FRMC";
pub const FORMAT_VERSION: u32 = 1;
pub const HEADER_LEN: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FileKind {
    Snapshot = 1,
    CommandLog = 2,
}

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("not a Formica save file (bad magic)")]
    BadMagic,
    #[error("file is too short")]
    Truncated,
    #[error("expected a {expected:?} file, found kind {found}")]
    WrongKind { expected: FileKind, found: u8 },
    #[error("unsupported format version {found} (this build reads {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("payload: {0}")]
    Payload(#[from] postcard::Error),
}

fn encode<T: Serialize>(kind: FileKind, value: &T) -> Result<Vec<u8>, SnapshotError> {
    let mut out = Vec::with_capacity(HEADER_LEN + 1024);
    out.extend_from_slice(&MAGIC);
    out.push(kind as u8);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    Ok(postcard::to_extend(value, out)?)
}

fn decode<T: DeserializeOwned>(kind: FileKind, bytes: &[u8]) -> Result<T, SnapshotError> {
    if bytes.len() < HEADER_LEN {
        return Err(SnapshotError::Truncated);
    }
    if bytes[..4] != MAGIC {
        return Err(SnapshotError::BadMagic);
    }
    if bytes[4] != kind as u8 {
        return Err(SnapshotError::WrongKind {
            expected: kind,
            found: bytes[4],
        });
    }
    let version = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(SnapshotError::UnsupportedVersion {
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    Ok(postcard::from_bytes(&bytes[HEADER_LEN..])?)
}

pub fn save_state(state: &SimState) -> Result<Vec<u8>, SnapshotError> {
    encode(FileKind::Snapshot, state)
}

pub fn load_state(bytes: &[u8]) -> Result<SimState, SnapshotError> {
    let mut state: SimState = decode(FileKind::Snapshot, bytes)?;
    state.world.rebuild_surface();
    Ok(state)
}

pub fn save_log(log: &CommandLog) -> Result<Vec<u8>, SnapshotError> {
    encode(FileKind::CommandLog, log)
}

pub fn load_log(bytes: &[u8]) -> Result<CommandLog, SnapshotError> {
    decode(FileKind::CommandLog, bytes)
}

/// `xxh3_64` of the canonical postcard encoding of the state.
pub fn state_hash(state: &SimState) -> u64 {
    let bytes = postcard::to_allocvec(state).expect("state is always serializable");
    xxhash_rust::xxh3::xxh3_64(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{Sim, SimCommand, SimConfig};

    #[test]
    fn snapshot_roundtrip_preserves_state_and_hash() {
        let mut s = Sim::new(SimConfig::test_chunk(3, 250));
        s.step_n(40);
        let bytes = save_state(s.state()).unwrap();
        assert_eq!(&bytes[..4], b"FRMC");
        let loaded = load_state(&bytes).unwrap();
        assert_eq!(&loaded, s.state());
        assert_eq!(state_hash(&loaded), s.state_hash());
        // Derived data is rebuilt on load.
        assert_eq!(loaded.world.surface_at(5, 5), s.world().surface_at(5, 5));
    }

    #[test]
    fn log_roundtrip() {
        let mut s = Sim::new(SimConfig::test_chunk(3, 10));
        s.queue(SimCommand::SpawnAgents { count: 3 });
        s.step_n(5);
        let log = s.command_log().unwrap();
        assert_eq!(load_log(&save_log(&log).unwrap()).unwrap(), log);
    }

    #[test]
    fn rejects_bad_files() {
        let s = Sim::new(SimConfig::test_chunk(3, 1));
        let good = save_state(s.state()).unwrap();
        assert!(matches!(load_state(b"FRM"), Err(SnapshotError::Truncated)));
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(matches!(load_state(&bad), Err(SnapshotError::BadMagic)));
        assert!(matches!(
            load_log(&good),
            Err(SnapshotError::WrongKind { .. })
        ));
        let mut future = good.clone();
        future[5..9].copy_from_slice(&99u32.to_le_bytes());
        assert!(matches!(
            load_state(&future),
            Err(SnapshotError::UnsupportedVersion { .. })
        ));
        assert!(matches!(
            load_state(&good[..good.len() - 10]),
            Err(SnapshotError::Payload(_))
        ));
    }
}

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bincode::{config, Decode, Encode};
use blake3::{Hash, Hasher, OUT_LEN};
use nockvm::pma::{
    classify_pma_noun, Pma, PmaDirectJamConfig, PmaDirectJamError, PmaDirectReader,
    PmaFileMetadata, PmaRawNounKind,
};
use nockvm::serialization::met0_u64_to_usize;
use thiserror::Error;
use tracing::info;

use crate::event_log::{EventLog, EventLogError, ReadySnapshotRecord};
use crate::utils::durability;

const SNAPSHOT_MANIFEST_MAGIC: u64 = u64::from_le_bytes(*b"SNAPMAN1");
const SNAPSHOT_MANIFEST_VERSION: u32 = 1;
type HashBytes = [u8; OUT_LEN];

fn duration_ms(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

#[derive(Clone, Copy, Debug, Encode, Decode, PartialEq, Eq)]
pub(crate) enum SnapshotKind {
    Epoch,
    Rotating,
}

#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq)]
struct SnapshotManifestPayload {
    magic: u64,
    version: u32,
    kind: SnapshotKind,
    timestamp_tag: String,
    ker_hash: HashBytes,
    event_num: u64,
    pma_words: u64,
    alloc_words: u64,
    kernel_root_raw: u64,
    cold_offset: u32,
    used_blake3: HashBytes,
    structure_blake3: Option<HashBytes>,
    created_at_ms: i64,
}

#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq)]
pub(crate) struct SnapshotManifest {
    pub magic: u64,
    pub version: u32,
    pub kind: SnapshotKind,
    pub timestamp_tag: String,
    pub ker_hash: HashBytes,
    pub event_num: u64,
    pub pma_words: u64,
    pub alloc_words: u64,
    pub kernel_root_raw: u64,
    pub cold_offset: u32,
    pub used_blake3: HashBytes,
    pub structure_blake3: Option<HashBytes>,
    pub created_at_ms: i64,
    pub checksum: HashBytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotVerifyMode {
    Fast,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JamStreamStats {
    pub bit_len: usize,
    pub byte_len: usize,
}

#[derive(Debug)]
pub(crate) struct SnapshotVerification {
    pub manifest: SnapshotManifest,
    pub file_metadata: PmaFileMetadata,
    pub used_blake3: HashBytes,
    pub structure_stats: Option<JamStreamStats>,
}

#[derive(Debug, Error)]
pub(crate) enum SnapshotManifestError {
    #[error("snapshot manifest io error: {0}")]
    Io(#[from] io::Error),
    #[error("snapshot manifest encode error: {0}")]
    Encode(#[from] bincode::error::EncodeError),
    #[error("snapshot manifest decode error: {0}")]
    Decode(#[from] bincode::error::DecodeError),
    #[error("snapshot manifest magic mismatch: expected {expected:#x}, found {found:#x}")]
    BadMagic { expected: u64, found: u64 },
    #[error("snapshot manifest version mismatch: expected {expected}, found {found}")]
    BadVersion { expected: u32, found: u32 },
    #[error("snapshot manifest checksum mismatch")]
    ChecksumMismatch,
}

#[derive(Debug, Error)]
pub(crate) enum SnapshotVerifyError {
    #[error(transparent)]
    Manifest(#[from] SnapshotManifestError),
    #[error("snapshot PMA metadata error: {0}")]
    Pma(#[from] nockvm::pma::PmaError),
    #[error("snapshot PMA direct-reader error: {0}")]
    Direct(#[from] PmaDirectJamError),
    #[error("snapshot io error: {0}")]
    Io(#[from] io::Error),
    #[error("snapshot PMA words mismatch: manifest={manifest}, file={file}")]
    PmaWordsMismatch { manifest: u64, file: u64 },
    #[error("snapshot alloc words mismatch: manifest={manifest}, file={file}")]
    AllocWordsMismatch { manifest: u64, file: u64 },
    #[error("snapshot used-range hash mismatch")]
    UsedHashMismatch {
        expected: HashBytes,
        actual: HashBytes,
    },
    #[error("snapshot structure hash mismatch")]
    StructureHashMismatch {
        expected: HashBytes,
        actual: HashBytes,
    },
    #[error("snapshot cold_offset {cold_offset} is out of bounds for alloc_words {alloc_words}")]
    ColdOffsetOutOfBounds { cold_offset: u32, alloc_words: u64 },
}

#[derive(Debug, Error)]
pub(crate) enum SnapshotBuildError {
    #[error(transparent)]
    Verify(#[from] SnapshotVerifyError),
    #[error(transparent)]
    Manifest(#[from] SnapshotManifestError),
    #[error(transparent)]
    EventLog(#[from] EventLogError),
    #[error("snapshot build io error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Error)]
pub(crate) enum SnapshotRestoreError {
    #[error(transparent)]
    Verify(#[from] SnapshotVerifyError),
    #[error("snapshot restore io error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Error)]
pub(crate) enum SnapshotCleanupError {
    #[error(transparent)]
    EventLog(#[from] EventLogError),
    #[error("snapshot cleanup io error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug)]
pub(crate) enum RotatingSnapshotBuildStatus {
    NotCreated,
    Created,
    CreatedWithCleanupError(SnapshotBuildError),
}

impl RotatingSnapshotBuildStatus {
    pub(crate) fn created(&self) -> bool {
        matches!(
            self,
            RotatingSnapshotBuildStatus::Created
                | RotatingSnapshotBuildStatus::CreatedWithCleanupError(_)
        )
    }

    pub(crate) fn cleanup_error(&self) -> Option<&SnapshotBuildError> {
        match self {
            RotatingSnapshotBuildStatus::CreatedWithCleanupError(err) => Some(err),
            _ => None,
        }
    }
}

impl SnapshotManifest {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        kind: SnapshotKind,
        timestamp_tag: String,
        ker_hash: Hash,
        event_num: u64,
        pma_words: u64,
        alloc_words: u64,
        kernel_root_raw: u64,
        cold_offset: u32,
        used_blake3: Hash,
        structure_blake3: Option<Hash>,
        created_at_ms: i64,
    ) -> Result<Self, SnapshotManifestError> {
        let mut manifest = Self {
            magic: SNAPSHOT_MANIFEST_MAGIC,
            version: SNAPSHOT_MANIFEST_VERSION,
            kind,
            timestamp_tag,
            ker_hash: to_hash_bytes(ker_hash),
            event_num,
            pma_words,
            alloc_words,
            kernel_root_raw,
            cold_offset,
            used_blake3: to_hash_bytes(used_blake3),
            structure_blake3: structure_blake3.map(to_hash_bytes),
            created_at_ms,
            checksum: [0; OUT_LEN],
        };
        manifest.checksum = manifest.compute_checksum()?;
        Ok(manifest)
    }

    pub(crate) fn validate(&self) -> Result<(), SnapshotManifestError> {
        if self.magic != SNAPSHOT_MANIFEST_MAGIC {
            return Err(SnapshotManifestError::BadMagic {
                expected: SNAPSHOT_MANIFEST_MAGIC,
                found: self.magic,
            });
        }
        if self.version != SNAPSHOT_MANIFEST_VERSION {
            return Err(SnapshotManifestError::BadVersion {
                expected: SNAPSHOT_MANIFEST_VERSION,
                found: self.version,
            });
        }
        if self.compute_checksum()? != self.checksum {
            return Err(SnapshotManifestError::ChecksumMismatch);
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, SnapshotManifestError> {
        self.validate()?;
        Ok(bincode::encode_to_vec(self, config::standard())?)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, SnapshotManifestError> {
        let (manifest, _) = bincode::decode_from_slice::<Self, _>(bytes, config::standard())?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub(crate) fn read_from_path(path: &Path) -> Result<Self, SnapshotManifestError> {
        let bytes = fs::read(path)?;
        Self::decode(&bytes)
    }

    pub(crate) fn write_to_path(&self, path: &Path) -> Result<(), SnapshotManifestError> {
        let bytes = self.encode()?;
        durability::write_atomic(path, &bytes, "snapshot_manifest_write")?;
        Ok(())
    }

    fn payload(&self) -> SnapshotManifestPayload {
        SnapshotManifestPayload {
            magic: self.magic,
            version: self.version,
            kind: self.kind,
            timestamp_tag: self.timestamp_tag.clone(),
            ker_hash: self.ker_hash,
            event_num: self.event_num,
            pma_words: self.pma_words,
            alloc_words: self.alloc_words,
            kernel_root_raw: self.kernel_root_raw,
            cold_offset: self.cold_offset,
            used_blake3: self.used_blake3,
            structure_blake3: self.structure_blake3,
            created_at_ms: self.created_at_ms,
        }
    }

    fn compute_checksum(&self) -> Result<HashBytes, SnapshotManifestError> {
        let payload = self.payload();
        Ok(compute_checksum(&payload)?)
    }
}

pub(crate) fn verify_snapshot(
    manifest_path: &Path,
    pma_path: &Path,
    mode: SnapshotVerifyMode,
) -> Result<SnapshotVerification, SnapshotVerifyError> {
    let verify_start = Instant::now();
    info!(
        mode = ?mode,
        manifest_path = ?manifest_path,
        pma_path = ?pma_path,
        "snapshot verify start"
    );
    let stage_start = Instant::now();
    let manifest = SnapshotManifest::read_from_path(manifest_path)?;
    info!(
        mode = ?mode,
        event_num = manifest.event_num,
        elapsed_ms = duration_ms(stage_start.elapsed()),
        "snapshot verify stage done: read_manifest"
    );
    let stage_start = Instant::now();
    let file_metadata = Pma::read_file_metadata(pma_path)?;
    info!(
        mode = ?mode,
        event_num = manifest.event_num,
        elapsed_ms = duration_ms(stage_start.elapsed()),
        "snapshot verify stage done: read_pma_metadata"
    );
    let stage_start = Instant::now();
    if manifest.pma_words != file_metadata.data_words {
        return Err(SnapshotVerifyError::PmaWordsMismatch {
            manifest: manifest.pma_words,
            file: file_metadata.data_words,
        });
    }
    if manifest.alloc_words != file_metadata.alloc_words {
        return Err(SnapshotVerifyError::AllocWordsMismatch {
            manifest: manifest.alloc_words,
            file: file_metadata.alloc_words,
        });
    }
    info!(
        mode = ?mode,
        event_num = manifest.event_num,
        elapsed_ms = duration_ms(stage_start.elapsed()),
        "snapshot verify stage done: validate_file_metadata"
    );

    let stage_start = Instant::now();
    let used_blake3 = hash_file_prefix(pma_path, file_metadata.alloc_words.saturating_mul(8))?;
    info!(
        mode = ?mode,
        event_num = manifest.event_num,
        elapsed_ms = duration_ms(stage_start.elapsed()),
        "snapshot verify stage done: hash_used_prefix"
    );
    if used_blake3 != manifest.used_blake3 {
        return Err(SnapshotVerifyError::UsedHashMismatch {
            expected: manifest.used_blake3,
            actual: used_blake3,
        });
    }

    let stage_start = Instant::now();
    let mut reader = PmaDirectReader::from_path(
        pma_path,
        file_metadata.data_words,
        file_metadata.alloc_words,
        PmaDirectJamConfig {
            require_direct_io: false,
            ..PmaDirectJamConfig::default()
        },
    )?;
    info!(
        mode = ?mode,
        event_num = manifest.event_num,
        elapsed_ms = duration_ms(stage_start.elapsed()),
        "snapshot verify stage done: open_direct_reader"
    );
    let stage_start = Instant::now();
    validate_root_raw(&mut reader, manifest.kernel_root_raw)?;
    info!(
        mode = ?mode,
        event_num = manifest.event_num,
        elapsed_ms = duration_ms(stage_start.elapsed()),
        "snapshot verify stage done: validate_root_raw"
    );
    let stage_start = Instant::now();
    validate_cold_offset(&mut reader, manifest.cold_offset)?;
    info!(
        mode = ?mode,
        event_num = manifest.event_num,
        elapsed_ms = duration_ms(stage_start.elapsed()),
        "snapshot verify stage done: validate_cold_offset"
    );

    let structure_stats = match mode {
        SnapshotVerifyMode::Fast => None,
        SnapshotVerifyMode::Full => {
            let stage_start = Instant::now();
            let stats = verify_structure(&mut reader, &manifest)?;
            info!(
                mode = ?mode,
                event_num = manifest.event_num,
                bit_len = stats.bit_len,
                byte_len = stats.byte_len,
                elapsed_ms = duration_ms(stage_start.elapsed()),
                "snapshot verify stage done: verify_structure"
            );
            Some(stats)
        }
    };

    let verification = SnapshotVerification {
        manifest,
        file_metadata,
        used_blake3,
        structure_stats,
    };
    info!(
        mode = ?mode,
        event_num = verification.manifest.event_num,
        elapsed_ms = duration_ms(verify_start.elapsed()),
        "snapshot verify done"
    );
    Ok(verification)
}

pub(crate) fn maybe_create_epoch_snapshot(
    event_log: &mut EventLog,
    pma: &Pma,
    ker_hash: Hash,
    event_num: u64,
    kernel_root_raw: u64,
    cold_offset: u32,
) -> Result<bool, SnapshotBuildError> {
    if event_log.has_ready_snapshot()? {
        return Ok(false);
    }
    create_ready_snapshot(
        event_log,
        pma,
        SnapshotKind::Epoch,
        "epoch".to_string(),
        "epoch".to_string(),
        ker_hash,
        event_num,
        kernel_root_raw,
        cold_offset,
        None,
    )?;
    Ok(true)
}

pub(crate) fn maybe_create_rotating_snapshot(
    event_log: &mut EventLog,
    pma: &Pma,
    ker_hash: Hash,
    event_num: u64,
    kernel_root_raw: u64,
    cold_offset: u32,
    cumulative_processing_time: Duration,
    rotating_snapshot_interval_time: Option<Duration>,
) -> Result<RotatingSnapshotBuildStatus, SnapshotBuildError> {
    let Some(interval) = rotating_snapshot_interval_time else {
        return Ok(RotatingSnapshotBuildStatus::NotCreated);
    };
    if cumulative_processing_time < interval {
        return Ok(RotatingSnapshotBuildStatus::NotCreated);
    }
    let created_at_ms = current_time_ms()?;
    let timestamp_tag = format!("{created_at_ms:020}-{event_num:020}");
    let file_stem = format!("snap-{timestamp_tag}");
    let base_snapshot_id = event_log.active_snapshot_id()?;
    create_ready_snapshot(
        event_log,
        pma,
        SnapshotKind::Rotating,
        timestamp_tag,
        file_stem,
        ker_hash,
        event_num,
        kernel_root_raw,
        cold_offset,
        base_snapshot_id,
    )?;
    if let Err(err) = retire_old_rotating_snapshots(event_log).map_err(snapshot_cleanup_into_build)
    {
        return Ok(RotatingSnapshotBuildStatus::CreatedWithCleanupError(err));
    }
    Ok(RotatingSnapshotBuildStatus::Created)
}

pub(crate) fn restore_verified_snapshot(
    record: &ReadySnapshotRecord,
    operative_pma_path: &Path,
) -> Result<SnapshotManifest, SnapshotRestoreError> {
    let verification = verify_snapshot(
        Path::new(&record.manifest_path),
        Path::new(&record.pma_path),
        SnapshotVerifyMode::Full,
    )?;
    let tmp_path = operative_pma_path.with_extension("restore.tmp");
    copy_snapshot_file(Path::new(&record.pma_path), &tmp_path)?;
    replace_file(&tmp_path, operative_pma_path)?;
    Ok(verification.manifest)
}

pub(crate) fn cleanup_snapshot_artifacts(
    event_log: &mut EventLog,
    pma_dir: &Path,
) -> Result<(), SnapshotCleanupError> {
    retire_old_rotating_snapshots(event_log)?;
    if !pma_dir.exists() {
        return Ok(());
    }

    let tracked_paths: HashSet<_> = event_log
        .list_ready_snapshots()?
        .into_iter()
        .flat_map(|snapshot| {
            [
                tracked_snapshot_path(pma_dir, &snapshot.pma_path),
                tracked_snapshot_path(pma_dir, &snapshot.manifest_path),
            ]
        })
        .collect();

    let corrupted_dir = pma_dir.join("corrupted_pma");
    for entry in fs::read_dir(pma_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_snapshot_artifact(name) || tracked_paths.contains(&path) {
            continue;
        }
        fs::create_dir_all(&corrupted_dir)?;
        move_to_corrupted_dir(&path, &corrupted_dir)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn create_ready_snapshot(
    event_log: &mut EventLog,
    pma: &Pma,
    kind: SnapshotKind,
    timestamp_tag: String,
    file_stem: String,
    ker_hash: Hash,
    event_num: u64,
    kernel_root_raw: u64,
    cold_offset: u32,
    base_snapshot_id: Option<i64>,
) -> Result<SnapshotManifest, SnapshotBuildError> {
    let pma_dir = pma.path().parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "PMA path missing parent directory for snapshot",
        )
    })?;
    fs::create_dir_all(pma_dir)?;
    let snapshot_pma_path = pma_dir.join(format!("{file_stem}.pma"));
    let snapshot_manifest_path = pma_dir.join(format!("{file_stem}.manifest"));
    let tmp_snapshot_pma_path = pma_dir.join(format!("{file_stem}.pma.tmp"));
    let created_at_ms = current_time_ms()?;

    pma.sync_used_data()?;
    pma.sync_trailer()?;
    durability::sync_path_data(pma.path(), "snapshot_source_pma_fdatasync")?;

    let copy_start = Instant::now();
    copy_snapshot_file(pma.path(), &tmp_snapshot_pma_path)?;
    info!(
        event_num,
        src = ?pma.path(),
        dst = ?tmp_snapshot_pma_path,
        elapsed_ms = duration_ms(copy_start.elapsed()),
        "snapshot file copy done"
    );
    replace_file(&tmp_snapshot_pma_path, &snapshot_pma_path)?;
    let hash_start = Instant::now();
    let used_blake3 = Hash::from_bytes(hash_file_prefix(
        &snapshot_pma_path,
        pma.alloc_offset() as u64 * 8,
    )?);
    info!(
        event_num,
        path = ?snapshot_pma_path,
        len_bytes = pma.alloc_offset() as u64 * 8,
        elapsed_ms = duration_ms(hash_start.elapsed()),
        "snapshot used-range hash done"
    );
    let manifest = SnapshotManifest::new(
        kind,
        timestamp_tag.clone(),
        ker_hash,
        event_num,
        pma.size_words() as u64,
        pma.alloc_offset() as u64,
        kernel_root_raw,
        cold_offset,
        used_blake3,
        None,
        created_at_ms,
    )?;
    manifest.write_to_path(&snapshot_manifest_path)?;

    let verify_start = Instant::now();
    if let Err(err) = verify_snapshot(
        &snapshot_manifest_path,
        &snapshot_pma_path,
        SnapshotVerifyMode::Fast,
    ) {
        let _ = fs::remove_file(&snapshot_manifest_path);
        let _ = fs::remove_file(&snapshot_pma_path);
        return Err(err.into());
    }
    info!(
        event_num,
        manifest_path = ?snapshot_manifest_path,
        pma_path = ?snapshot_pma_path,
        elapsed_ms = duration_ms(verify_start.elapsed()),
        "snapshot verify build stage done"
    );

    event_log.insert_ready_snapshot(&ReadySnapshotRecord {
        snapshot_id: 0,
        kind: snapshot_kind_name(kind).to_string(),
        event_num,
        pma_path: snapshot_pma_path.to_string_lossy().into_owned(),
        manifest_path: snapshot_manifest_path.to_string_lossy().into_owned(),
        alloc_words: pma.alloc_offset() as u64,
        kernel_root_raw,
        cold_offset,
        used_blake3: manifest.used_blake3.to_vec(),
        structure_blake3: manifest.structure_blake3.map(|hash| hash.to_vec()),
        created_at_ms,
        activated_at_ms: Some(created_at_ms),
        base_snapshot_id,
        timestamp_tag,
    })?;
    Ok(manifest)
}

fn retire_old_rotating_snapshots(event_log: &mut EventLog) -> Result<(), SnapshotCleanupError> {
    let retire_start = Instant::now();
    let rotating = event_log.ready_rotating_snapshots()?;
    let mut retired_count = 0usize;
    for snapshot in rotating.into_iter().skip(2) {
        let pma_path = Path::new(&snapshot.pma_path);
        if pma_path.exists() {
            fs::remove_file(pma_path)?;
        }
        let manifest_path = Path::new(&snapshot.manifest_path);
        if manifest_path.exists() {
            fs::remove_file(manifest_path)?;
        }
        event_log.retire_snapshot(snapshot.snapshot_id)?;
        retired_count += 1;
    }
    info!(
        retired_count,
        elapsed_ms = duration_ms(retire_start.elapsed()),
        "retire old rotating snapshots done"
    );
    Ok(())
}

fn snapshot_cleanup_into_build(err: SnapshotCleanupError) -> SnapshotBuildError {
    match err {
        SnapshotCleanupError::EventLog(err) => SnapshotBuildError::EventLog(err),
        SnapshotCleanupError::Io(err) => SnapshotBuildError::Io(err),
    }
}

fn tracked_snapshot_path(pma_dir: &Path, raw: &str) -> std::path::PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        pma_dir.join(path)
    }
}

fn is_snapshot_artifact(name: &str) -> bool {
    matches!(
        name,
        "epoch.pma" | "epoch.manifest" | "epoch.tmp" | "epoch.pma.tmp" | "epoch.manifest.tmp"
    ) || name.starts_with("snap-")
}

fn move_to_corrupted_dir(path: &Path, corrupted_dir: &Path) -> Result<(), io::Error> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "snapshot artifact missing filename",
            )
        })?;
    let mut target = corrupted_dir.join(file_name);
    let mut suffix = 1usize;
    while target.exists() {
        target = corrupted_dir.join(format!("{file_name}.{suffix}"));
        suffix = suffix.saturating_add(1);
    }
    fs::rename(path, &target)?;
    sync_parent_dir(&target)?;
    Ok(())
}

fn snapshot_kind_name(kind: SnapshotKind) -> &'static str {
    match kind {
        SnapshotKind::Epoch => "epoch",
        SnapshotKind::Rotating => "rotating",
    }
}

fn validate_root_raw(
    reader: &mut PmaDirectReader,
    kernel_root_raw: u64,
) -> Result<(), SnapshotVerifyError> {
    match classify_pma_noun(kernel_root_raw)? {
        PmaRawNounKind::Direct(_) => Ok(()),
        PmaRawNounKind::Indirect { offset } => {
            let _ = reader.indirect_atom_words(offset)?;
            Ok(())
        }
        PmaRawNounKind::Cell { offset } => {
            let _ = reader.read_cell(offset)?;
            Ok(())
        }
    }
}

fn validate_cold_offset(
    reader: &mut PmaDirectReader,
    cold_offset: u32,
) -> Result<(), SnapshotVerifyError> {
    let alloc_words = reader.alloc_words();
    if u64::from(cold_offset) >= alloc_words {
        return Err(SnapshotVerifyError::ColdOffsetOutOfBounds {
            cold_offset,
            alloc_words,
        });
    }
    let _ = reader.read_u64(u64::from(cold_offset))?;
    Ok(())
}

fn verify_structure(
    reader: &mut PmaDirectReader,
    manifest: &SnapshotManifest,
) -> Result<JamStreamStats, SnapshotVerifyError> {
    let mut writer = StructureBitWriter::new(manifest.structure_blake3.is_some());
    let mut backrefs: HashMap<u64, usize> = HashMap::new();
    let mut stack = vec![manifest.kernel_root_raw];

    while let Some(noun_raw) = stack.pop() {
        let kind = classify_pma_noun(noun_raw)?;
        if let Some(backref) = backrefs.get(&noun_raw).copied() {
            match kind {
                PmaRawNounKind::Direct(value) => {
                    let atom_bits = met0_u64_to_usize(value);
                    if met0_u64_to_usize(backref as u64) < atom_bits {
                        mat_backref(&mut writer, backref)?;
                    } else {
                        mat_direct_atom(&mut writer, value)?;
                    }
                }
                PmaRawNounKind::Indirect { offset } => {
                    let atom_bits = reader.indirect_atom_bits(offset)?;
                    if met0_u64_to_usize(backref as u64) < atom_bits {
                        mat_backref(&mut writer, backref)?;
                    } else {
                        mat_indirect_atom(reader, &mut writer, offset, atom_bits)?;
                    }
                }
                PmaRawNounKind::Cell { .. } => {
                    mat_backref(&mut writer, backref)?;
                }
            }
            continue;
        }

        backrefs.insert(noun_raw, writer.bit_len());
        match kind {
            PmaRawNounKind::Direct(value) => {
                mat_direct_atom(&mut writer, value)?;
            }
            PmaRawNounKind::Indirect { offset } => {
                let atom_bits = reader.indirect_atom_bits(offset)?;
                mat_indirect_atom(reader, &mut writer, offset, atom_bits)?;
            }
            PmaRawNounKind::Cell { offset } => {
                let (head, tail) = reader.read_cell(offset)?;
                mat_cell(&mut writer)?;
                stack.push(tail);
                stack.push(head);
            }
        }
    }

    let (stats, actual_hash) = writer.finish();
    if let Some(expected) = manifest.structure_blake3 {
        let actual = actual_hash.expect("structure hash requested");
        if actual != expected {
            return Err(SnapshotVerifyError::StructureHashMismatch { expected, actual });
        }
    }
    Ok(stats)
}

fn compute_checksum<T: Encode>(value: &T) -> Result<HashBytes, SnapshotManifestError> {
    let encoded = bincode::encode_to_vec(value, config::standard())?;
    let mut hasher = Hasher::new();
    hasher.update(&encoded);
    Ok(*hasher.finalize().as_bytes())
}

fn to_hash_bytes(hash: Hash) -> HashBytes {
    *hash.as_bytes()
}

fn hash_file_prefix(path: &Path, len_bytes: u64) -> Result<HashBytes, io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Hasher::new();
    let mut remaining = len_bytes;
    let mut buf = [0u8; 8192];
    while remaining > 0 {
        let read_len = remaining.min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..read_len])?;
        hasher.update(&buf[..read_len]);
        remaining -= read_len as u64;
    }
    Ok(*hasher.finalize().as_bytes())
}

fn sync_parent_dir(path: &Path) -> Result<(), io::Error> {
    durability::sync_parent_dir(path, "snapshot_parent_dir_fsync")
}

fn copy_snapshot_file(src: &Path, dst: &Path) -> Result<(), io::Error> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dst)?;
    let file = File::open(dst)?;
    durability::sync_all(&file, "snapshot_file_fsync", Some(dst))?;
    Ok(())
}

fn replace_file(src: &Path, dst: &Path) -> Result<(), io::Error> {
    if dst.exists() {
        fs::remove_file(dst)?;
    }
    fs::rename(src, dst)?;
    sync_parent_dir(dst)?;
    Ok(())
}

fn current_time_ms() -> Result<i64, io::Error> {
    Ok(i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
            .as_millis(),
    )
    .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?)
}

struct StructureBitWriter {
    hasher: Option<Hasher>,
    current_byte: u8,
    current_bits: u8,
    bit_len: usize,
}

impl StructureBitWriter {
    fn new(hash_output: bool) -> Self {
        Self {
            hasher: hash_output.then(Hasher::new),
            current_byte: 0,
            current_bits: 0,
            bit_len: 0,
        }
    }

    fn bit_len(&self) -> usize {
        self.bit_len
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        if let Some(hasher) = self.hasher.as_mut() {
            hasher.update(bytes);
        }
    }

    fn push_zero_bytes(&mut self, mut count: usize) {
        const ZERO_BUF: [u8; 4096] = [0; 4096];
        while count > 0 {
            let chunk = count.min(ZERO_BUF.len());
            self.push_bytes(&ZERO_BUF[..chunk]);
            count -= chunk;
        }
    }

    fn flush_current_byte(&mut self) {
        if self.current_bits == 0 {
            return;
        }
        self.push_bytes(&[self.current_byte]);
        self.current_byte = 0;
        self.current_bits = 0;
    }

    fn write_bit(&mut self, bit: bool) {
        if bit {
            self.current_byte |= 1u8 << self.current_bits;
        }
        self.current_bits += 1;
        self.bit_len += 1;
        if self.current_bits == 8 {
            self.flush_current_byte();
        }
    }

    fn write_zeros(&mut self, count: usize) {
        let mut remaining = count;
        if self.current_bits == 0 {
            let full_bytes = remaining / 8;
            if full_bytes > 0 {
                self.push_zero_bytes(full_bytes);
                self.bit_len += full_bytes * 8;
                remaining -= full_bytes * 8;
            }
        }
        for _ in 0..remaining {
            self.write_bit(false);
        }
    }

    fn write_bits_from_value(&mut self, mut value: u64, bits: usize) {
        let mut remaining = bits;
        if self.current_bits == 0 {
            while remaining >= 8 {
                self.push_bytes(&[value as u8]);
                self.bit_len += 8;
                value >>= 8;
                remaining -= 8;
            }
        }
        for _ in 0..remaining {
            self.write_bit((value & 1) != 0);
            value >>= 1;
        }
    }

    fn finish(mut self) -> (JamStreamStats, Option<HashBytes>) {
        self.flush_current_byte();
        let byte_len = self.bit_len.div_ceil(8);
        let hash = self
            .hasher
            .take()
            .map(|hasher| *hasher.finalize().as_bytes());
        (
            JamStreamStats {
                bit_len: self.bit_len,
                byte_len,
            },
            hash,
        )
    }
}

fn mat_backref(writer: &mut StructureBitWriter, backref: usize) -> Result<(), PmaDirectJamError> {
    if backref == 0 {
        writer.write_bits_from_value(0b111, 3);
        return Ok(());
    }
    let backref_sz = met0_u64_to_usize(backref as u64);
    let backref_sz_sz = met0_u64_to_usize(backref_sz as u64);
    writer.write_bit(true);
    writer.write_bit(true);
    writer.write_zeros(backref_sz_sz);
    writer.write_bit(true);
    writer.write_bits_from_value(backref_sz as u64, backref_sz_sz - 1);
    writer.write_bits_from_value(backref as u64, backref_sz);
    Ok(())
}

fn mat_direct_atom(writer: &mut StructureBitWriter, value: u64) -> Result<(), PmaDirectJamError> {
    if value == 0 {
        writer.write_bits_from_value(0b10, 2);
        return Ok(());
    }
    let atom_bits = met0_u64_to_usize(value);
    mat_atom_header(writer, atom_bits)?;
    writer.write_bits_from_value(value, atom_bits);
    Ok(())
}

fn mat_indirect_atom(
    reader: &mut PmaDirectReader,
    writer: &mut StructureBitWriter,
    offset: u64,
    atom_bits: usize,
) -> Result<(), PmaDirectJamError> {
    if atom_bits == 0 {
        writer.write_bits_from_value(0b10, 2);
        return Ok(());
    }
    let size_words = reader.indirect_atom_words(offset)?;
    mat_atom_header(writer, atom_bits)?;
    let last_bits = atom_bits.saturating_sub((size_words - 1).saturating_mul(64));
    for i in 0..size_words {
        let word = reader.read_u64(offset + 2 + i as u64)?;
        let bits = if i + 1 == size_words { last_bits } else { 64 };
        writer.write_bits_from_value(word, bits);
    }
    Ok(())
}

fn mat_atom_header(
    writer: &mut StructureBitWriter,
    atom_bits: usize,
) -> Result<(), PmaDirectJamError> {
    let atom_sz_sz = met0_u64_to_usize(atom_bits as u64);
    writer.write_bit(false);
    writer.write_zeros(atom_sz_sz);
    writer.write_bit(true);
    writer.write_bits_from_value(atom_bits as u64, atom_sz_sz - 1);
    Ok(())
}

fn mat_cell(writer: &mut StructureBitWriter) -> Result<(), PmaDirectJamError> {
    writer.write_bits_from_value(0b01, 2);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;

    use nockvm::jets::cold::Cold;
    use nockvm::mem::NockStack;
    use nockvm::noun::{Noun, D, T};
    use nockvm::pma::{classify_pma_noun, Pma, PmaCopy, PmaRawNounKind};
    use tempfile::TempDir;

    use super::*;
    use crate::test_support::native_pma_test_guard;

    fn build_test_snapshot() -> (TempDir, PathBuf, PathBuf, SnapshotManifest, u64) {
        let temp = TempDir::new().expect("tempdir");
        let pma_path = temp.path().join("snapshot.pma");
        let manifest_path = temp.path().join("snapshot.manifest");
        let mut pma = Pma::new(4096, pma_path.clone()).expect("new pma");
        let mut stack = NockStack::new(1 << 16, 0);
        let mut cold = Cold::new(&mut stack);
        let mut root: Noun = T(&mut stack, &[D(42), D(0)]);
        unsafe {
            cold.copy_to_pma(&stack, &mut pma);
            root.copy_to_pma(&stack, &mut pma);
        }
        let cold_offset = cold.pma_offset(&pma).expect("cold offset");
        let root_raw = unsafe { root.as_raw() };
        pma.sync_used_data().expect("sync used");
        pma.sync_trailer().expect("sync trailer");
        durability::sync_path_data(pma.path(), "snapshot_test_source_fdatasync")
            .expect("sync file");
        let manifest = SnapshotManifest::new(
            SnapshotKind::Epoch,
            "epoch".to_string(),
            blake3::hash(b"kernel"),
            7,
            pma.size_words() as u64,
            pma.alloc_offset() as u64,
            root_raw,
            cold_offset,
            Hash::from_bytes(
                hash_file_prefix(&pma_path, pma.alloc_offset() as u64 * 8).expect("used hash"),
            ),
            None,
            1234,
        )
        .expect("manifest");
        manifest
            .write_to_path(&manifest_path)
            .expect("write manifest");
        drop(pma);
        (temp, pma_path, manifest_path, manifest, root_raw)
    }

    fn overwrite_u64(path: &Path, offset_words: u64, value: u64) {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("open pma");
        file.seek(SeekFrom::Start(offset_words * 8))
            .expect("seek pma");
        file.write_all(&value.to_ne_bytes()).expect("write word");
        durability::sync_all(&file, "snapshot_test_file_fsync", None).expect("sync file");
    }

    #[test]
    fn manifest_roundtrips_with_checksum() {
        let manifest = SnapshotManifest::new(
            SnapshotKind::Rotating,
            "snap-123".to_string(),
            blake3::hash(b"ker"),
            11,
            1024,
            64,
            0,
            3,
            blake3::hash(b"used"),
            Some(blake3::hash(b"struct")),
            99,
        )
        .expect("manifest");
        let encoded = manifest.encode().expect("encode");
        let decoded = SnapshotManifest::decode(&encoded).expect("decode");
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn verify_snapshot_accepts_valid_snapshot() {
        let _guard = native_pma_test_guard();
        let (_temp, pma_path, manifest_path, manifest, _root_raw) = build_test_snapshot();
        let verification =
            verify_snapshot(&manifest_path, &pma_path, SnapshotVerifyMode::Full).expect("verify");
        assert_eq!(verification.manifest, manifest);
        assert_eq!(verification.file_metadata.alloc_words, manifest.alloc_words);
        assert!(verification.structure_stats.is_some());
    }

    #[test]
    fn verify_snapshot_rejects_used_hash_mismatch() {
        let _guard = native_pma_test_guard();
        let (_temp, pma_path, manifest_path, mut manifest, _root_raw) = build_test_snapshot();
        manifest.used_blake3 = [7; OUT_LEN];
        manifest.checksum = manifest.compute_checksum().expect("checksum");
        manifest
            .write_to_path(&manifest_path)
            .expect("write manifest");

        let err = verify_snapshot(&manifest_path, &pma_path, SnapshotVerifyMode::Fast)
            .expect_err("mismatch");
        assert!(matches!(err, SnapshotVerifyError::UsedHashMismatch { .. }));
    }

    #[test]
    fn verify_snapshot_rejects_structural_corruption() {
        let _guard = native_pma_test_guard();
        let (_temp, pma_path, manifest_path, mut manifest, root_raw) = build_test_snapshot();
        let root_offset = match classify_pma_noun(root_raw).expect("classify root") {
            PmaRawNounKind::Cell { offset } => offset,
            other => panic!("expected cell root, got {other:?}"),
        };
        overwrite_u64(&pma_path, root_offset + 1, u64::MAX);
        manifest.used_blake3 =
            hash_file_prefix(&pma_path, manifest.alloc_words * 8).expect("rehash used range");
        manifest.checksum = manifest.compute_checksum().expect("checksum");
        manifest
            .write_to_path(&manifest_path)
            .expect("write manifest");

        let err = verify_snapshot(&manifest_path, &pma_path, SnapshotVerifyMode::Full)
            .expect_err("corrupt");
        assert!(matches!(err, SnapshotVerifyError::Direct(_)));
    }
}

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::store::{EventRecord, EventStore, SnapshotRecord};
use crate::workspace::Workspace;

const DEFAULT_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct CaptureOptions {
    pub label: Option<String>,
    pub agent_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub max_file_bytes: u64,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            label: None,
            agent_id: None,
            dispatch_id: None,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceEntry {
    pub relative_path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
}

pub trait EvidenceWorkspace {
    fn backend_name(&self) -> &'static str;
    fn capture_root(&self) -> String;
    fn list_entries(&self) -> Result<Vec<SourceEntry>>;
    fn list_skipped(&self) -> Result<Vec<ManifestSkipped>> {
        Ok(Vec::new())
    }
    fn read_bytes(&self, relative_path: &str) -> Result<Vec<u8>>;
}

pub trait SnapshotStore {
    fn write_blob(&self, sha256_hex: &str, bytes: &[u8]) -> Result<String>;
    fn write_manifest(&self, snapshot_id: &str, manifest_bytes: &[u8]) -> Result<String>;
}

pub struct LocalSnapshotStore<'a> {
    workspace: &'a Workspace,
}

impl<'a> LocalSnapshotStore<'a> {
    pub fn new(workspace: &'a Workspace) -> Self {
        Self { workspace }
    }
}

impl SnapshotStore for LocalSnapshotStore<'_> {
    fn write_blob(&self, sha: &str, bytes: &[u8]) -> Result<String> {
        let (prefix, rest) = sha.split_at(2);
        let dir = self.workspace.blobs_dir().join(prefix);
        fs::create_dir_all(&dir)?;
        let path = dir.join(rest);
        if !path.exists() {
            fs::write(&path, bytes).with_context(|| format!("write blob {}", path.display()))?;
        }
        path_relative_to(&path, &self.workspace.root)
    }

    fn write_manifest(&self, snapshot_id: &str, manifest_bytes: &[u8]) -> Result<String> {
        let snapshot_dir = self.workspace.snapshots_dir().join(snapshot_id);
        fs::create_dir_all(&snapshot_dir)
            .with_context(|| format!("create snapshot dir {}", snapshot_dir.display()))?;
        let path = snapshot_dir.join("manifest.json");
        fs::write(&path, manifest_bytes)
            .with_context(|| format!("write manifest {}", path.display()))?;
        path_relative_to(&path, &self.workspace.root)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub snapshot_id: String,
    pub event_id: String,
    pub backend: String,
    pub capture_root: String,
    pub created_at: DateTime<Utc>,
    pub label: Option<String>,
    pub agent_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub files: Vec<ManifestFile>,
    pub skipped: Vec<ManifestSkipped>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime: DateTime<Utc>,
    pub sha256: String,
    pub blob_ref: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestSkipped {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct CaptureProtocol {
    pub snapshot_id: String,
    pub event_id: String,
    pub manifest_hash: String,
    pub manifest_path: String,
    pub backend: String,
    pub capture_root: String,
    pub file_count: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct CaptureDisplay {
    pub summary: String,
    pub label: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotListProtocol {
    pub snapshots: Vec<SnapshotSummaryProtocol>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotSummaryProtocol {
    pub snapshot_id: String,
    pub event_id: String,
    pub manifest_hash: String,
    pub backend: String,
    pub capture_root: String,
    pub created_at: DateTime<Utc>,
    pub file_count: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct SnapshotListDisplay {
    pub summary: String,
}

#[derive(Debug, Serialize)]
pub struct SnapshotShowProtocol {
    pub snapshot_id: String,
    pub event_id: String,
    pub manifest_hash: String,
    pub manifest_path: String,
    pub backend: String,
    pub capture_root: String,
    pub created_at: DateTime<Utc>,
    pub label: Option<String>,
    pub agent_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub file_count: u64,
    pub total_bytes: u64,
    pub files: Vec<ManifestFile>,
    pub skipped: Vec<ManifestSkipped>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotShowDisplay {
    pub summary: String,
    pub label: Option<String>,
}

pub struct SnapshotCapture<'a, W: EvidenceWorkspace, S: SnapshotStore> {
    source: &'a W,
    snapshot_store: &'a S,
    store: &'a EventStore,
}

impl<'a, W: EvidenceWorkspace, S: SnapshotStore> SnapshotCapture<'a, W, S> {
    pub fn new(source: &'a W, snapshot_store: &'a S, store: &'a EventStore) -> Self {
        Self {
            source,
            snapshot_store,
            store,
        }
    }

    pub fn capture(&self, options: CaptureOptions) -> Result<SnapshotRecord> {
        let snapshot_id = prefixed_id("snap");
        let event_id = prefixed_id("evt");
        let created_at = Utc::now();

        let mut files = Vec::new();
        let mut skipped = self.source.list_skipped()?;
        let mut total_bytes = 0_u64;

        for entry in self.source.list_entries()? {
            if entry.kind != EntryKind::File {
                continue;
            }
            if entry.size > options.max_file_bytes {
                skipped.push(ManifestSkipped {
                    path: entry.relative_path,
                    reason: "file_too_large".to_string(),
                });
                continue;
            }
            match self.source.read_bytes(&entry.relative_path) {
                Ok(bytes) => {
                    let sha = sha256_hex(&bytes);
                    let blob_ref = self.snapshot_store.write_blob(&sha, &bytes)?;
                    total_bytes += entry.size;
                    files.push(ManifestFile {
                        path: entry.relative_path,
                        kind: entry.kind,
                        size: entry.size,
                        mtime: entry.mtime,
                        sha256: format!("sha256:{sha}"),
                        blob_ref,
                    });
                }
                Err(error) => skipped.push(ManifestSkipped {
                    path: entry.relative_path,
                    reason: format!("read_failed:{error}"),
                }),
            }
        }

        files.sort_by(|left, right| left.path.cmp(&right.path));
        skipped.sort_by(|left, right| left.path.cmp(&right.path));

        let manifest = SnapshotManifest {
            snapshot_id: snapshot_id.clone(),
            event_id: event_id.clone(),
            backend: self.source.backend_name().to_string(),
            capture_root: self.source.capture_root(),
            created_at,
            label: options.label.clone(),
            agent_id: options.agent_id.clone(),
            dispatch_id: options.dispatch_id.clone(),
            files,
            skipped,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        let manifest_hash = format!("sha256:{}", sha256_hex(&manifest_bytes));
        let manifest_rel = self
            .snapshot_store
            .write_manifest(&snapshot_id, &manifest_bytes)?;
        let record = SnapshotRecord {
            snapshot_id,
            event_id,
            created_at,
            backend: self.source.backend_name().to_string(),
            capture_root: self.source.capture_root(),
            label: options.label,
            agent_id: options.agent_id,
            dispatch_id: options.dispatch_id,
            manifest_path: manifest_rel,
            manifest_hash,
            file_count: manifest.files.len() as u64,
            total_bytes,
        };

        let event = EventRecord {
            event_id: record.event_id.clone(),
            event_type: "evidence.snapshot_captured".to_string(),
            created_at: record.created_at,
            payload: json!({
                "snapshot_id": record.snapshot_id,
                "backend": record.backend,
                "capture_root": record.capture_root,
                "manifest_path": record.manifest_path,
                "manifest_hash": record.manifest_hash,
                "file_count": record.file_count,
                "total_bytes": record.total_bytes,
                "agent_id": record.agent_id,
                "dispatch_id": record.dispatch_id,
                "label": record.label,
            }),
        };
        self.store.insert_event(&event)?;
        self.store.insert_snapshot(&record)?;

        Ok(record)
    }
}

#[derive(Clone, Debug)]
pub struct LocalFsEvidenceWorkspace {
    root: PathBuf,
    scope: PathBuf,
}

impl LocalFsEvidenceWorkspace {
    pub fn new(workspace_root: &Path, scope: &Path) -> Result<Self> {
        let root = workspace_root
            .canonicalize()
            .with_context(|| format!("canonicalize workspace root {}", workspace_root.display()))?;
        let scope = if scope.is_absolute() {
            scope.to_path_buf()
        } else {
            root.join(scope)
        };
        if !scope.exists() {
            return Err(anyhow!("capture path does not exist: {}", scope.display()));
        }
        let scope = scope
            .canonicalize()
            .with_context(|| format!("canonicalize capture scope {}", scope.display()))?;
        if !scope.starts_with(&root) {
            return Err(anyhow!(
                "capture path must stay inside workspace: {}",
                scope.display()
            ));
        }
        Ok(Self { root, scope })
    }
}

impl EvidenceWorkspace for LocalFsEvidenceWorkspace {
    fn backend_name(&self) -> &'static str {
        "local"
    }

    fn capture_root(&self) -> String {
        self.scope.display().to_string()
    }

    fn list_entries(&self) -> Result<Vec<SourceEntry>> {
        if self.scope.is_file() {
            let metadata = fs::metadata(&self.scope)?;
            let mtime = metadata
                .modified()
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            return Ok(vec![SourceEntry {
                relative_path: path_relative_to(&self.scope, &self.root)?,
                kind: EntryKind::File,
                size: metadata.len(),
                mtime,
            }]);
        }

        let mut entries = Vec::new();
        for entry in WalkDir::new(&self.scope)
            .into_iter()
            .filter_entry(|entry| !should_ignore_path(entry.path()))
        {
            let entry = entry?;
            if entry.file_type().is_symlink() {
                continue;
            }
            let path = entry.path();
            if path == self.scope {
                continue;
            }
            let metadata = entry.metadata()?;
            let relative_path = path_relative_to(path, &self.root)?;
            if metadata.is_dir() {
                continue;
            }
            let mtime = metadata
                .modified()
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            entries.push(SourceEntry {
                relative_path,
                kind: EntryKind::File,
                size: metadata.len(),
                mtime,
            });
        }
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(entries)
    }

    fn list_skipped(&self) -> Result<Vec<ManifestSkipped>> {
        if self.scope.is_file() {
            return Ok(Vec::new());
        }

        let mut skipped = Vec::new();
        for entry in WalkDir::new(&self.scope)
            .min_depth(1)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if entry.file_type().is_symlink() && !ancestor_is_ignored(path, &self.scope) {
                skipped.push(ManifestSkipped {
                    path: path_relative_to(path, &self.root)?,
                    reason: "symlink_unsupported".to_string(),
                });
            } else if should_ignore_path(path) && !ancestor_is_ignored(path, &self.scope) {
                skipped.push(ManifestSkipped {
                    path: path_relative_to(path, &self.root)?,
                    reason: "ignored".to_string(),
                });
            }
        }
        skipped.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(skipped)
    }

    fn read_bytes(&self, relative_path: &str) -> Result<Vec<u8>> {
        let path = self.root.join(relative_path);
        if !path.starts_with(&self.root) {
            return Err(anyhow!("path escapes workspace: {relative_path}"));
        }
        fs::read(&path).with_context(|| format!("read {}", path.display()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct MemoryEvidenceWorkspace {
    capture_root: String,
    files: BTreeMap<String, Vec<u8>>,
    mtimes: BTreeMap<String, DateTime<Utc>>,
}

impl MemoryEvidenceWorkspace {
    pub fn new(capture_root: impl Into<String>) -> Self {
        Self {
            capture_root: capture_root.into(),
            files: BTreeMap::new(),
            mtimes: BTreeMap::new(),
        }
    }

    pub fn add_file(&mut self, path: impl Into<String>, bytes: impl Into<Vec<u8>>) {
        let path = path.into();
        self.files.insert(path.clone(), bytes.into());
        self.mtimes.insert(path, Utc::now());
    }
}

impl EvidenceWorkspace for MemoryEvidenceWorkspace {
    fn backend_name(&self) -> &'static str {
        "memory"
    }

    fn capture_root(&self) -> String {
        self.capture_root.clone()
    }

    fn list_entries(&self) -> Result<Vec<SourceEntry>> {
        Ok(self
            .files
            .iter()
            .map(|(path, bytes)| SourceEntry {
                relative_path: path.clone(),
                kind: EntryKind::File,
                size: bytes.len() as u64,
                mtime: self.mtimes.get(path).copied().unwrap_or_else(Utc::now),
            })
            .collect())
    }

    fn read_bytes(&self, relative_path: &str) -> Result<Vec<u8>> {
        self.files
            .get(relative_path)
            .cloned()
            .ok_or_else(|| anyhow!("missing memory file: {relative_path}"))
    }
}

pub fn read_manifest(workspace: &Workspace, snapshot: &SnapshotRecord) -> Result<SnapshotManifest> {
    let path = workspace.root.join(&snapshot.manifest_path);
    let bytes = fs::read(&path).with_context(|| format!("read manifest {}", path.display()))?;
    let actual_hash = format!("sha256:{}", sha256_hex(&bytes));
    if actual_hash != snapshot.manifest_hash {
        return Err(anyhow!(
            "manifest hash mismatch for {}: expected {}, got {}",
            snapshot.snapshot_id,
            snapshot.manifest_hash,
            actual_hash
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn should_ignore_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".rive" | ".git" | "target" | "node_modules" | ".next" | ".cache" | "dist" | "build"
        )
    })
}

fn ancestor_is_ignored(path: &Path, stop_at: &Path) -> bool {
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent == stop_at {
            return false;
        }
        if should_ignore_path(parent) {
            return true;
        }
        current = parent.parent();
    }
    false
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn sha256_hex<R: AsRef<[u8]>>(bytes: R) -> String {
    let mut hasher = Sha256::new();
    let mut cursor = Cursor::new(bytes.as_ref());
    let mut buffer = [0_u8; 8192];
    loop {
        let read = cursor.read(&mut buffer).expect("cursor read cannot fail");
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hex::encode(hasher.finalize())
}

fn path_relative_to(path: &Path, base: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(base)
        .with_context(|| format!("{} is not inside {}", path.display(), base.display()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::workspace::init_workspace;

    use super::*;

    #[derive(Default)]
    struct MemorySnapshotStore {
        blobs: std::cell::RefCell<BTreeMap<String, Vec<u8>>>,
        manifests: std::cell::RefCell<BTreeMap<String, Vec<u8>>>,
    }

    impl SnapshotStore for MemorySnapshotStore {
        fn write_blob(&self, sha256_hex: &str, bytes: &[u8]) -> Result<String> {
            self.blobs
                .borrow_mut()
                .insert(sha256_hex.to_string(), bytes.to_vec());
            Ok(format!("memory://blobs/{sha256_hex}"))
        }

        fn write_manifest(&self, snapshot_id: &str, manifest_bytes: &[u8]) -> Result<String> {
            self.manifests
                .borrow_mut()
                .insert(snapshot_id.to_string(), manifest_bytes.to_vec());
            Ok(format!("memory://snapshots/{snapshot_id}/manifest.json"))
        }
    }

    #[test]
    fn capture_uses_evidence_workspace_trait() {
        let temp = TempDir::new().unwrap();
        let workspace = init_workspace(temp.path()).unwrap();
        let store = EventStore::open(&workspace.db_path()).unwrap();
        let mut memory = MemoryEvidenceWorkspace::new("memory://case");
        memory.add_file("src/main.rs", b"fn main() {}\n".to_vec());
        let snapshot_store = LocalSnapshotStore::new(&workspace);

        let capture = SnapshotCapture::new(&memory, &snapshot_store, &store);
        let snapshot = capture
            .capture(CaptureOptions {
                label: Some("memory-test".to_string()),
                ..CaptureOptions::default()
            })
            .unwrap();

        let manifest = read_manifest(&workspace, &snapshot).unwrap();
        assert_eq!(snapshot.backend, "memory");
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "src/main.rs");
        assert!(manifest.files[0].sha256.starts_with("sha256:"));
    }

    #[test]
    fn capture_skips_large_files() {
        let temp = TempDir::new().unwrap();
        let workspace = init_workspace(temp.path()).unwrap();
        let store = EventStore::open(&workspace.db_path()).unwrap();
        let mut memory = MemoryEvidenceWorkspace::new("memory://large");
        memory.add_file("big.bin", vec![1_u8, 2, 3, 4]);
        let snapshot_store = LocalSnapshotStore::new(&workspace);

        let capture = SnapshotCapture::new(&memory, &snapshot_store, &store);
        let snapshot = capture
            .capture(CaptureOptions {
                max_file_bytes: 2,
                ..CaptureOptions::default()
            })
            .unwrap();

        let manifest = read_manifest(&workspace, &snapshot).unwrap();
        assert_eq!(manifest.files.len(), 0);
        assert_eq!(manifest.skipped.len(), 1);
        assert_eq!(manifest.skipped[0].reason, "file_too_large");
    }

    #[test]
    fn capture_can_write_through_snapshot_store_trait() {
        let temp = TempDir::new().unwrap();
        let workspace = init_workspace(temp.path()).unwrap();
        let store = EventStore::open(&workspace.db_path()).unwrap();
        let mut memory = MemoryEvidenceWorkspace::new("memory://source");
        memory.add_file("note.txt", b"hello".to_vec());
        let snapshot_store = MemorySnapshotStore::default();

        let capture = SnapshotCapture::new(&memory, &snapshot_store, &store);
        let snapshot = capture.capture(CaptureOptions::default()).unwrap();

        assert_eq!(snapshot.backend, "memory");
        assert!(snapshot.manifest_path.starts_with("memory://snapshots/"));
        assert_eq!(snapshot_store.blobs.borrow().len(), 1);
        assert_eq!(snapshot_store.manifests.borrow().len(), 1);
    }

    #[test]
    fn local_fs_records_ignored_paths_as_skipped() {
        let temp = TempDir::new().unwrap();
        let workspace = init_workspace(temp.path()).unwrap();
        fs::write(workspace.root.join("keep.txt"), "keep").unwrap();
        fs::create_dir_all(workspace.root.join(".git/objects")).unwrap();
        fs::write(workspace.root.join(".git/config"), "ignored").unwrap();
        fs::create_dir_all(workspace.root.join("node_modules/pkg")).unwrap();
        fs::write(workspace.root.join("node_modules/pkg/index.js"), "ignored").unwrap();
        let store = EventStore::open(&workspace.db_path()).unwrap();
        let source = LocalFsEvidenceWorkspace::new(&workspace.root, &workspace.root).unwrap();
        let snapshot_store = LocalSnapshotStore::new(&workspace);

        let capture = SnapshotCapture::new(&source, &snapshot_store, &store);
        let snapshot = capture.capture(CaptureOptions::default()).unwrap();
        let manifest = read_manifest(&workspace, &snapshot).unwrap();

        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "keep.txt");
        assert!(manifest
            .skipped
            .iter()
            .any(|item| item.path == ".git" && item.reason == "ignored"));
        assert!(manifest
            .skipped
            .iter()
            .any(|item| item.path == "node_modules" && item.reason == "ignored"));
    }
}

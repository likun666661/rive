use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::store::EventStore;

pub const RIVE_DIR: &str = ".rive";
pub const DB_FILE: &str = "rive.db";

#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    pub fn rive_dir(&self) -> PathBuf {
        self.root.join(RIVE_DIR)
    }

    pub fn db_path(&self) -> PathBuf {
        self.rive_dir().join(DB_FILE)
    }

    pub fn evidence_dir(&self) -> PathBuf {
        self.rive_dir().join("evidence")
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        self.evidence_dir().join("snapshots")
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.evidence_dir().join("blobs")
    }

    pub fn artifacts_dir(&self) -> PathBuf {
        self.rive_dir().join("artifacts")
    }

    pub fn debug_dir(&self) -> PathBuf {
        self.rive_dir().join("debug")
    }

    pub fn debug_trace_dir(&self) -> PathBuf {
        self.debug_dir().join("trace")
    }

    pub fn debug_trace_payloads_dir(&self) -> PathBuf {
        self.debug_trace_dir().join("payloads")
    }

    pub fn debug_runs_dir(&self) -> PathBuf {
        self.debug_dir().join("runs")
    }
}

pub fn init_workspace(path: &Path) -> Result<Workspace> {
    fs::create_dir_all(path).with_context(|| format!("create workspace {}", path.display()))?;
    let root = path
        .canonicalize()
        .with_context(|| format!("canonicalize workspace {}", path.display()))?;
    let workspace = Workspace { root };

    fs::create_dir_all(workspace.snapshots_dir())?;
    fs::create_dir_all(workspace.blobs_dir())?;
    fs::create_dir_all(workspace.artifacts_dir())?;
    fs::create_dir_all(workspace.debug_trace_payloads_dir())?;
    fs::create_dir_all(workspace.debug_runs_dir())?;
    fs::create_dir_all(workspace.rive_dir().join("run"))?;

    write_if_missing(&workspace.rive_dir().join("tasks.md"), "# Rive Tasks\n")?;
    write_if_missing(
        &workspace.rive_dir().join("PROTOCOL.md"),
        "# Rive Protocol\n\nThis workspace stores Rive snapshot evidence in `.rive/`.\n",
    )?;

    let store = EventStore::open(&workspace.db_path())?;
    store.init_schema()?;

    Ok(workspace)
}

pub fn find_workspace(start: &Path) -> Result<Workspace> {
    let canonical = start
        .canonicalize()
        .with_context(|| format!("canonicalize {}", start.display()))?;
    let mut current = if canonical.is_file() {
        canonical
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("path has no parent: {}", start.display()))?
    } else {
        canonical
    };

    loop {
        if current.join(RIVE_DIR).is_dir() {
            return Ok(Workspace { root: current });
        }
        if !current.pop() {
            break;
        }
    }

    Err(anyhow!(
        "no .rive workspace found from {}; run `rive init` first",
        start.display()
    ))
}

fn write_if_missing(path: &Path, content: &str) -> Result<()> {
    if !path.exists() {
        fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(())
}

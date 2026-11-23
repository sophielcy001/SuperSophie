// SPDX-License-Identifier: AGPL-3.0-or-later

mod package;
mod scheduled;

pub use package::{RecoveryKind, RecoveryPoint};
pub use scheduled::{BackupAttempt, BackupCheck, BackupPolicy, BackupRunState, BackupStatus};

use crate::store::now;
use crate::{Error, Result, Store};
use package::{Capture, create_package};
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

impl Store {
    pub fn save_recovery_package(
        &self,
        workspace: &str,
        destination: &Path,
    ) -> Result<RecoveryPoint> {
        let parent = destination
            .parent()
            .ok_or_else(bad_destination)?
            .canonicalize()?;
        let filename = destination.file_name().ok_or_else(bad_destination)?;
        let target = parent.join(filename);
        // Keep workspace data, configuration and migration evidence protected.
        if parent.starts_with(self.data_root())
            || fs::symlink_metadata(&target).is_ok_and(|item| !item.is_file())
        {
            return Err(bad_destination());
        }
        let db = self.open_workspace_db(workspace, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        create_package(
            self,
            &db,
            workspace,
            &target,
            Capture::new(RecoveryKind::Manual, now()),
        )
    }

    /// Accept both the portable content archive and a disaster-recovery package.
    pub fn read_workspace_package(&self, mut input: impl Read + Seek) -> Result<crate::Workspace> {
        input.rewind()?;
        let mut zip = ZipArchive::new(&mut input)?;
        if zip.index_for_name(package::MANIFEST).is_some() {
            return package::restore_package(self, &mut zip);
        }
        drop(zip);
        self.read_workspace_archive(input)
    }
}

pub(crate) fn preserve_upgrade(
    store: &Store,
    workspace: &str,
    version: u32,
    snapshot: &Path,
) -> Result<()> {
    let db = Connection::open_with_flags(snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let target = snapshot.with_file_name(format!("{workspace}-v{version}.zhiye-recovery.zip"));
    create_package(
        store,
        &db,
        workspace,
        &target,
        Capture::new(RecoveryKind::Upgrade, now()),
    )?;
    Ok(())
}

pub(super) fn bad_destination() -> Error {
    Error::new("BACKUP_PATH", "请选择工作空间数据目录以外的文件位置。")
}

pub(super) fn managed_directory(root: &Path, name: &str) -> Result<PathBuf> {
    let directory = root.join(name);
    fs::create_dir_all(&directory)?;
    if directory.canonicalize()? != directory {
        return Err(Error::new("BACKUP_PATH", "备份目录不能指向其他位置。"));
    }
    Ok(directory)
}

pub(super) fn persist_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let parent = path.parent().ok_or_else(bad_destination)?;
    let mut staging = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut staging, value)?;
    staging.write_all(b"\n")?;
    staging.as_file().sync_all()?;
    staging
        .persist(path)
        .map_err(|error| Error::from(error.error))?;
    Ok(())
}

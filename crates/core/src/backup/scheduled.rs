// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{bad_destination, managed_directory, package, persist_json};
use crate::model::valid_id;
use crate::store::now;
use crate::{Error, RecoveryKind, RecoveryPoint, Result, Store};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::TryLockError,
    time::{Duration, UNIX_EPOCH},
};

const POLICY: &str = "backup-policy.json";
const JOURNALS: &str = "backup-state";
const BACKUPS: &str = "backups";
const DAY: i64 = 24 * 60 * 60 * 1_000;
const RETRY: i64 = 5 * 60 * 1_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupPolicy {
    pub enabled: bool,
    pub directory: Option<PathBuf>,
    pub keep_daily: u16,
    pub keep_weekly: u16,
}

impl Default for BackupPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: None,
            keep_daily: 7,
            keep_weekly: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupRunState {
    Running,
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupAttempt {
    pub workspace_id: String,
    pub id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub state: BackupRunState,
    pub directory: PathBuf,
    pub error: Option<String>,
    session: String,
}

pub struct BackupCheck {
    pub workspace_id: String,
    pub result: Result<Option<BackupAttempt>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub policy: BackupPolicy,
    pub directory: PathBuf,
    pub default_directory: PathBuf,
    pub last_attempt: Option<BackupAttempt>,
    pub last_success: Option<RecoveryPoint>,
    pub check_error: Option<String>,
    pub retention_error: Option<String>,
    pub upgrade_directory: PathBuf,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    last_attempt: Option<BackupAttempt>,
    points: Vec<JournalPoint>,
    retention_error: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct JournalPoint {
    #[serde(flatten)]
    recovery: RecoveryPoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verified_modified: Option<Duration>,
}

impl JournalPoint {
    fn verified(recovery: RecoveryPoint) -> Self {
        let verified_modified = point_metadata(&recovery.path)
            .ok()
            .filter(|metadata| metadata.is_file() && metadata.len() == recovery.bytes)
            .and_then(|metadata| modified_since_epoch(&metadata));
        Self {
            recovery,
            verified_modified,
        }
    }
}

impl Store {
    pub fn backup_status(&self, workspace: Option<&str>) -> Result<BackupStatus> {
        let policy = load::<BackupPolicy>(&self.data_root().join(POLICY))?;
        let directory = backup_directory(self.data_root(), &policy);
        let state: Journal = match workspace {
            Some(workspace) => load(&journal_path(self.data_root(), workspace)?)?,
            None => Journal::default(),
        };
        let last_attempt = state.last_attempt.map(|mut attempt| {
            if attempt.state == BackupRunState::Running
                && (attempt.session != self.session_id || self.backup_gate.try_lock().is_ok())
            {
                attempt.state = BackupRunState::Interrupted;
            }
            attempt
        });
        let check_error = match workspace {
            Some(id) => self
                .backup_check_errors
                .lock()
                .map_err(|_| backup_state_error())?
                .get(id)
                .cloned(),
            None => None,
        };
        Ok(BackupStatus {
            default_directory: self.data_root().join(BACKUPS),
            policy,
            directory,
            last_attempt,
            last_success: state
                .points
                .into_iter()
                .map(|entry| entry.recovery)
                .max_by_key(|point| point.created_at),
            check_error,
            retention_error: state.retention_error,
            upgrade_directory: self.data_root().join("upgrade-backups"),
        })
    }

    pub fn set_backup_policy(&self, mut policy: BackupPolicy) -> Result<BackupPolicy> {
        let _coordinator = self.backup_gate.try_lock().map_err(coordinator_error)?;
        if !(1..=365).contains(&policy.keep_daily) || policy.keep_weekly > 52 {
            return Err(Error::new(
                "BACKUP_POLICY",
                "每日保留范围为 1 至 365 份，每周保留范围为 0 至 52 份。",
            ));
        }
        let current: BackupPolicy = load(&self.data_root().join(POLICY))?;
        if let Some(chosen) = &policy.directory
            && policy.directory != current.directory
        {
            let resolved = chosen.canonicalize()?;
            if !resolved.is_dir() || resolved.starts_with(self.data_root()) {
                return Err(bad_destination());
            }
            policy.directory = Some(resolved);
        }
        persist_json(&self.data_root().join(POLICY), &policy)?;
        Ok(policy)
    }

    /// Check each workspace independently, including workspaces with no backup due.
    pub fn run_due_backups(&self) -> Result<Vec<BackupCheck>> {
        let _coordinator = match self.backup_gate.try_lock() {
            Ok(lock) => lock,
            Err(TryLockError::WouldBlock) => return Ok(vec![]),
            Err(error) => return Err(coordinator_error(error)),
        };
        let policy = load::<BackupPolicy>(&self.data_root().join(POLICY))?;
        if !policy.enabled {
            return Ok(vec![]);
        }
        let mut checks = Vec::new();
        for workspace in self.workspace_ids()? {
            let result = self.run_backup_check(&workspace, &policy, false);
            checks.push(BackupCheck {
                workspace_id: workspace,
                result,
            });
        }
        Ok(checks)
    }

    pub fn run_workspace_backup(&self, workspace: &str) -> Result<Option<BackupAttempt>> {
        let _coordinator = self.backup_gate.try_lock().map_err(coordinator_error)?;
        let policy = load::<BackupPolicy>(&self.data_root().join(POLICY))?;
        if !valid_id(workspace) || !self.data_root().join(workspace).is_dir() {
            return Err(Error::new("WORKSPACE_NOT_FOUND", "工作空间不存在。"));
        }
        self.run_backup_check(workspace, &policy, true)
    }

    fn run_backup_check(
        &self,
        workspace: &str,
        policy: &BackupPolicy,
        force: bool,
    ) -> Result<Option<BackupAttempt>> {
        let result = run(self, workspace, policy, now(), force);
        // If the journal cannot be read or written, keep the current failure
        // visible until a later check can use the journal again.
        let mut errors = self
            .backup_check_errors
            .lock()
            .map_err(|_| backup_state_error())?;
        match &result {
            Ok(_) => {
                errors.remove(workspace);
            }
            Err(error) => {
                errors.insert(workspace.into(), error.message.clone());
            }
        }
        result
    }
}

fn backup_state_error() -> Error {
    Error::new("STORE_STATE", "备份服务需要重新启动。")
}

fn coordinator_error<T>(error: TryLockError<T>) -> Error {
    match error {
        TryLockError::WouldBlock => {
            Error::new("BACKUP_BUSY", "一个自动备份正在运行，请在完成后重试。")
        }
        TryLockError::Poisoned(_) => backup_state_error(),
    }
}

fn journal_path(root: &Path, workspace: &str) -> Result<PathBuf> {
    if !valid_id(workspace) {
        return Err(Error::new("WORKSPACE_NOT_FOUND", "工作空间不存在。"));
    }
    Ok(root.join(JOURNALS).join(format!("{workspace}.json")))
}

fn load<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= 32 * 1024 * 1024 => {}
        Ok(_) => return Err(Error::new("BACKUP_STATE", "备份设置或记录文件无法读取。")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error.into()),
    }
    if path.canonicalize()? != path {
        return Err(Error::new(
            "BACKUP_STATE",
            "备份设置或记录不能指向其他位置。",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn backup_directory(root: &Path, policy: &BackupPolicy) -> PathBuf {
    policy
        .directory
        .clone()
        .unwrap_or_else(|| root.join(BACKUPS))
}

fn verify_directory(store: &Store, directory: &Path) -> Result<()> {
    if !directory.is_dir()
        || directory.canonicalize()? != directory
        || (directory.starts_with(store.data_root())
            && directory != store.data_root().join(BACKUPS))
    {
        return Err(Error::new(
            "BACKUP_PATH",
            "备份目录不可用或已指向其他位置。",
        ));
    }
    Ok(())
}

fn automatic_folder(directory: &Path, workspace: &str, id: &str, at: i64) -> PathBuf {
    let date = DateTime::<Utc>::from_timestamp_millis(at)
        .map(|time| time.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| at.to_string());
    directory.join(format!("知页-{date}-{workspace}-{id}"))
}

fn automatic_path(directory: &Path, workspace: &str, id: &str, at: i64) -> PathBuf {
    automatic_folder(directory, workspace, id, at).join("知页整库恢复.zhiye-recovery.zip")
}

fn run(
    store: &Store,
    workspace: &str,
    policy: &BackupPolicy,
    at: i64,
    force: bool,
) -> Result<Option<BackupAttempt>> {
    let directory = backup_directory(store.data_root(), policy);
    managed_directory(store.data_root(), JOURNALS)?;
    let journal = journal_path(store.data_root(), workspace)?;
    let mut state: Journal = load(&journal)?;
    if reconcile(store, workspace, &mut state, at) {
        // Record the recovered package before retention removes an older copy.
        persist_json(&journal, &state)?;
        if let Some(attempt) = &state.last_attempt
            && attempt.state == BackupRunState::Completed
            && attempt.directory == directory
        {
            prune(store, workspace, &directory, policy, &mut state);
            persist_json(&journal, &state)?;
        }
    }
    if !force
        && state.last_attempt.as_ref().is_some_and(|attempt| {
            attempt.directory == directory
                && attempt.state == BackupRunState::Failed
                && attempt
                    .finished_at
                    .is_some_and(|time| at < time.saturating_add(RETRY))
        })
    {
        return Ok(None);
    }
    if verify_directory(store, &directory).is_ok() {
        let mut updated = false;
        let reusable = state
            .points
            .iter_mut()
            .filter(|entry| {
                entry.recovery.created_at.div_euclid(DAY) == at.div_euclid(DAY)
                    && matching_point(&entry.recovery, workspace, &directory)
            })
            .any(|entry| {
                let previous = entry.verified_modified;
                let valid = point_metadata(&entry.recovery.path).is_ok_and(|metadata| {
                    check_point(entry, workspace, &directory, &metadata, force).is_ok()
                });
                updated |= previous != entry.verified_modified;
                valid
            });
        if reusable {
            if let Some(previous_error) = state.retention_error.take() {
                let points_before = state.points.len();
                prune(store, workspace, &directory, policy, &mut state);
                updated |= state.points.len() != points_before
                    || state.retention_error.as_ref() != Some(&previous_error);
            }
            if updated {
                persist_json(&journal, &state)?;
            }
            return Ok(None);
        }
    }
    let capture = package::Capture::new(RecoveryKind::Automatic, at);
    let target = automatic_path(&directory, workspace, &capture.id, capture.at);
    let mut attempt = BackupAttempt {
        workspace_id: workspace.into(),
        id: capture.id.clone(),
        started_at: at,
        finished_at: None,
        state: BackupRunState::Running,
        directory,
        error: None,
        session: store.session_id.clone(),
    };
    state.last_attempt = Some(attempt.clone());
    persist_json(&journal, &state)?;
    let created = (|| {
        if policy.directory.is_none() {
            managed_directory(store.data_root(), BACKUPS)?;
        }
        verify_directory(store, &attempt.directory)?;
        let db = store.open_workspace_db(workspace, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let folder = target.parent().ok_or_else(bad_destination)?;
        // Claim a unique folder before publishing its ZIP so plain atomic
        // rename works on volumes without exclusive rename or hard links.
        fs::create_dir(folder)?;
        let saved = package::create_package(store, &db, workspace, &target, capture);
        if saved.is_err() {
            let _ = fs::remove_dir(folder);
        }
        saved
    })();
    attempt.finished_at = Some(now().max(at));
    match created {
        Ok(point) => {
            state.points.push(JournalPoint::verified(point));
            attempt.state = BackupRunState::Completed;
        }
        Err(error) => {
            attempt.state = BackupRunState::Failed;
            attempt.error = Some(error.message);
        }
    }
    state.last_attempt = Some(attempt.clone());
    // The successful publication is recorded before any old copy is pruned.
    persist_json(&journal, &state)?;
    if attempt.state == BackupRunState::Completed {
        prune(store, workspace, &attempt.directory, policy, &mut state);
        persist_json(&journal, &state)?;
    }
    Ok(Some(attempt))
}

fn reconcile(store: &Store, workspace: &str, journal: &mut Journal, at: i64) -> bool {
    let Some(attempt) = journal.last_attempt.as_mut() else {
        return false;
    };
    if attempt.state != BackupRunState::Running {
        return false;
    }
    let found = (|| {
        if attempt.workspace_id != workspace || !valid_id(&attempt.id) {
            return Err(Error::new("BACKUP_STATE", "备份记录与工作空间不符。"));
        }
        verify_directory(store, &attempt.directory)?;
        let target = automatic_path(
            &attempt.directory,
            workspace,
            &attempt.id,
            attempt.started_at,
        );
        point_metadata(&target)?;
        package::inspect_automatic(&target, workspace, &attempt.id, true)
    })();
    attempt.finished_at = Some(at);
    match found {
        Ok(point) => {
            if !journal
                .points
                .iter()
                .any(|entry| entry.recovery.id == point.id)
            {
                journal.points.push(JournalPoint::verified(point));
            }
            attempt.state = BackupRunState::Completed;
        }
        Err(_) => {
            attempt.state = BackupRunState::Interrupted;
            attempt.error = Some("上一次自动备份在确认完成前中断。".into());
        }
    }
    true
}

fn matching_point(point: &RecoveryPoint, workspace: &str, directory: &Path) -> bool {
    point.kind == RecoveryKind::Automatic
        && valid_id(&point.id)
        && point.workspace_id == workspace
        && point.path == automatic_path(directory, workspace, &point.id, point.created_at)
}

fn point_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let folder = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    if folder.canonicalize()? != folder {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "旧备份目录已指向其他位置。",
        ));
    }
    fs::symlink_metadata(path)
}

fn modified_since_epoch(metadata: &fs::Metadata) -> Option<Duration> {
    metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()
}

fn changed_point() -> Error {
    Error::new("BACKUP_RETENTION", "一份旧备份的原始文件已变化。")
}

fn check_point(
    entry: &mut JournalPoint,
    workspace: &str,
    directory: &Path,
    metadata: &fs::Metadata,
    force: bool,
) -> Result<()> {
    let point = &entry.recovery;
    if !matching_point(point, workspace, directory)
        || !metadata.is_file()
        || metadata.len() != point.bytes
    {
        entry.verified_modified = None;
        return Err(changed_point());
    }
    let modified = modified_since_epoch(metadata);
    // The background poll can reuse the result while the file metadata matches.
    if force || modified.is_none() || entry.verified_modified != modified {
        entry.verified_modified = None;
        let actual = package::inspect_automatic(&point.path, workspace, &point.id, true)?;
        if actual.created_at != point.created_at || actual.bytes != point.bytes {
            return Err(changed_point());
        }
        entry.verified_modified = modified;
    }
    Ok(())
}

fn prune(
    store: &Store,
    workspace: &str,
    directory: &Path,
    policy: &BackupPolicy,
    journal: &mut Journal,
) {
    journal.retention_error = None;
    if let Err(error) = verify_directory(store, directory) {
        journal.retention_error = Some(error.message);
        return;
    }
    journal
        .points
        .sort_by_key(|entry| std::cmp::Reverse(entry.recovery.created_at));
    let mut daily = BTreeSet::new();
    let mut weekly = BTreeSet::new();
    journal.points.retain_mut(|entry| {
        let point = &entry.recovery;
        if point.path.parent().and_then(Path::parent) != Some(directory)
            || point.workspace_id != workspace
        {
            return true;
        }
        let metadata = match point_metadata(&point.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
            Err(error) => {
                journal.retention_error = Some(Error::from(error).message);
                return true;
            }
        };
        if let Err(error) = check_point(entry, workspace, directory, &metadata, false) {
            journal.retention_error = Some(error.message);
            return true;
        }
        let point = &entry.recovery;
        let day = point.created_at.div_euclid(DAY);
        let keep_day = daily.len() < usize::from(policy.keep_daily) && daily.insert(day);
        let keep_week =
            DateTime::<Utc>::from_timestamp_millis(point.created_at).is_some_and(|time| {
                let week = time.iso_week();
                weekly.len() < usize::from(policy.keep_weekly)
                    && weekly.insert((week.year(), week.week()))
            });
        if keep_day || keep_week {
            return true;
        }
        let remove = (|| {
            let actual = package::inspect_automatic(&point.path, workspace, &point.id, false)?;
            if actual.created_at != point.created_at || actual.bytes != point.bytes {
                return Err(changed_point());
            }
            fs::remove_file(&point.path)?;
            if let Some(folder) = point.path.parent() {
                let _ = fs::remove_dir(folder);
            }
            Ok(())
        })();
        if let Err(error) = remove {
            journal.retention_error = Some(error.message);
            true
        } else {
            false
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io::{Seek, SeekFrom, Write};

    fn jan(day: u32) -> i64 {
        Utc.with_ymd_and_hms(2030, 1, day, 12, 0, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn same_length_corrupt_snapshot_does_not_count_as_todays_backup() {
        for force in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::open(root.path()).unwrap();
            let workspace = store.create_workspace("恢复包完整性").unwrap().id;
            let policy = BackupPolicy {
                keep_daily: 2,
                keep_weekly: 0,
                ..BackupPolicy::default()
            };
            run(&store, &workspace, &policy, jan(1), false)
                .unwrap()
                .unwrap();
            let previous = store
                .backup_status(Some(&workspace))
                .unwrap()
                .last_success
                .unwrap();
            let page = store
                .create_page(&workspace, None, "今天的内容", "file")
                .unwrap();
            run(&store, &workspace, &policy, jan(2), false)
                .unwrap()
                .unwrap();
            let damaged = store
                .backup_status(Some(&workspace))
                .unwrap()
                .last_success
                .unwrap();
            let modified = fs::metadata(&damaged.path).unwrap().modified().unwrap();
            let mut archive = zip::ZipArchive::new(File::open(&damaged.path).unwrap()).unwrap();
            let offset = archive
                .by_name("workspace.sqlite")
                .unwrap()
                .data_start()
                .expect("the SQLite entry has a data offset");
            drop(archive);
            let mut file = fs::OpenOptions::new()
                .write(true)
                .open(&damaged.path)
                .unwrap();
            file.seek(SeekFrom::Start(offset)).unwrap();
            file.write_all(b"!").unwrap();
            file.sync_all().unwrap();
            let observed_modified = if force {
                modified
            } else {
                modified + Duration::from_secs(2)
            };
            file.set_times(fs::FileTimes::new().set_modified(observed_modified))
                .unwrap();
            drop(file);

            let metadata = fs::metadata(&damaged.path).unwrap();
            assert_eq!(metadata.len(), damaged.bytes);
            assert_eq!(metadata.modified().unwrap(), observed_modified);
            assert!(
                package::inspect_automatic(&damaged.path, &workspace, &damaged.id, false).is_ok()
            );
            assert!(
                package::inspect_automatic(&damaged.path, &workspace, &damaged.id, true).is_err()
            );
            let retry = run(&store, &workspace, &policy, jan(2) + 1, force)
                .unwrap()
                .expect("a corrupt archive must not satisfy the daily backup check");
            assert_eq!(retry.state, BackupRunState::Completed);
            assert_ne!(retry.id, damaged.id);
            assert!(previous.path.is_file());
            assert!(damaged.path.is_file());
            let status = store.backup_status(Some(&workspace)).unwrap();
            assert!(status.retention_error.is_some());
            let replacement = status.last_success.unwrap();
            assert_eq!(replacement.id, retry.id);
            let restored = store
                .read_workspace_package(File::open(replacement.path).unwrap())
                .unwrap();
            assert_eq!(
                store.page(&restored.id, &page.info.id).unwrap().info.title,
                "今天的内容"
            );
        }
    }

    #[test]
    fn revalidates_a_backup_from_a_journal_without_the_verification_timestamp() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let workspace = store.create_workspace("旧备份记录").unwrap().id;
        let policy = BackupPolicy::default();
        let attempt = run(&store, &workspace, &policy, jan(1), false)
            .unwrap()
            .unwrap();
        let path = journal_path(store.data_root(), &workspace).unwrap();
        let mut legacy: serde_json::Value = load(&path).unwrap();
        let entry = &mut legacy["points"][0];
        let point: RecoveryPoint = serde_json::from_value(entry.clone()).unwrap();
        assert_eq!(point.id, attempt.id);
        assert!(
            entry
                .as_object_mut()
                .unwrap()
                .remove("verifiedModified")
                .is_some()
        );
        persist_json(&path, &legacy).unwrap();
        drop(store);

        let reopened = Store::open(root.path()).unwrap();
        assert!(
            run(&reopened, &workspace, &policy, jan(1) + 1, false)
                .unwrap()
                .is_none()
        );
        let journal: Journal = load(&path).unwrap();
        assert_eq!(journal.points.len(), 1);
        assert_eq!(journal.points[0].recovery.id, attempt.id);
        assert!(journal.points[0].verified_modified.is_some());
    }

    #[test]
    fn daily_and_weekly_retention_are_applied_only_to_managed_points() {
        let root = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let workspace = store.create_workspace("留存测试").unwrap().id;
        let policy = store
            .set_backup_policy(BackupPolicy {
                directory: Some(output.path().into()),
                keep_daily: 2,
                keep_weekly: 2,
                ..BackupPolicy::default()
            })
            .unwrap();
        let directory = policy.directory.as_ref().unwrap();
        let manual = directory.join("manual.zhiye-recovery.zip");
        store.save_recovery_package(&workspace, &manual).unwrap();
        for day in [1, 2, 7, 8, 9, 14, 15, 16] {
            let attempt = run(&store, &workspace, &policy, jan(day), false)
                .unwrap()
                .unwrap();
            assert_eq!(attempt.state, BackupRunState::Completed);
            assert!(
                run(&store, &workspace, &policy, jan(day) + 1, true)
                    .unwrap()
                    .is_none()
            );
        }
        let journal: Journal = load(&journal_path(store.data_root(), &workspace).unwrap()).unwrap();
        let kept: Vec<_> = journal
            .points
            .iter()
            .map(|entry| entry.recovery.created_at)
            .collect();
        assert_eq!(kept, [jan(16), jan(15), jan(9)]);
        assert!(journal.retention_error.is_none());
        assert_eq!(fs::read_dir(directory).unwrap().count(), 4);
        assert!(manual.is_file());
    }

    #[test]
    fn offline_destination_reports_failure_and_retries_without_deleting_the_last_copy() {
        let root = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("connected");
        let detached = parent.path().join("disconnected");
        fs::create_dir(&output).unwrap();
        let store = Store::open(root.path()).unwrap();
        let workspace = store.create_workspace("目录离线").unwrap().id;
        let policy = store
            .set_backup_policy(BackupPolicy {
                directory: Some(output.clone()),
                keep_daily: 1,
                keep_weekly: 0,
                ..BackupPolicy::default()
            })
            .unwrap();
        run(&store, &workspace, &policy, jan(1), false)
            .unwrap()
            .unwrap();
        let status = store.backup_status(Some(&workspace)).unwrap();
        let first = status.last_success.unwrap().path;
        let relative = first
            .strip_prefix(policy.directory.as_deref().unwrap())
            .unwrap();
        fs::rename(&output, &detached).unwrap();
        let failed = run(&store, &workspace, &policy, jan(2), false)
            .unwrap()
            .unwrap();
        assert_eq!(failed.state, BackupRunState::Failed);
        assert_eq!(
            store
                .backup_status(Some(&workspace))
                .unwrap()
                .last_success
                .unwrap()
                .path,
            first
        );
        assert!(
            run(&store, &workspace, &policy, jan(2) + 1, false)
                .unwrap()
                .is_none()
        );
        assert!(detached.join(relative).is_file());
        store
            .set_backup_policy(BackupPolicy {
                enabled: false,
                ..policy.clone()
            })
            .unwrap();
        assert!(store.run_due_backups().unwrap().is_empty());
        fs::rename(&detached, &output).unwrap();
        fs::write(&first, "别的程序保存的文件").unwrap();
        let retry = run(&store, &workspace, &policy, jan(2) + 1, true)
            .unwrap()
            .unwrap();
        assert_eq!(retry.state, BackupRunState::Completed);
        assert_eq!(fs::read_to_string(first).unwrap(), "别的程序保存的文件");
        assert!(
            store
                .backup_status(Some(&workspace))
                .unwrap()
                .retention_error
                .is_some()
        );
    }

    #[cfg(windows)]
    #[test]
    fn reused_backup_retries_retention_after_a_windows_file_lock() {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 1;
        for force in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::open(root.path()).unwrap();
            let workspace = store.create_workspace("重试清理旧备份").unwrap().id;
            let policy = BackupPolicy {
                keep_daily: 1,
                keep_weekly: 0,
                ..BackupPolicy::default()
            };
            run(&store, &workspace, &policy, jan(1), false).unwrap();
            let previous = store
                .backup_status(Some(&workspace))
                .unwrap()
                .last_success
                .unwrap();
            let previous_bytes = fs::read(&previous.path).unwrap();
            let locked = fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&previous.path)
                .unwrap();
            let page = store
                .create_page(&workspace, None, "较新快照中的页面", "file")
                .unwrap();
            let attempt = run(&store, &workspace, &policy, jan(2), false)
                .unwrap()
                .unwrap();
            assert_eq!(attempt.state, BackupRunState::Completed);
            let before_retry = store.backup_status(Some(&workspace)).unwrap();
            let latest = before_retry.last_success.unwrap();
            assert_eq!(latest.id, attempt.id);
            assert!(before_retry.retention_error.is_some());
            let latest_bytes = fs::read(&latest.path).unwrap();
            assert!(
                run(&store, &workspace, &policy, jan(2) + 1, force)
                    .unwrap()
                    .is_none()
            );
            assert!(
                store
                    .backup_status(Some(&workspace))
                    .unwrap()
                    .retention_error
                    .is_some()
            );
            assert_eq!(fs::read(&previous.path).unwrap(), previous_bytes);
            assert_eq!(fs::read(&latest.path).unwrap(), latest_bytes);
            drop(locked);
            drop(store);

            let reopened = Store::open(root.path()).unwrap();
            assert!(
                run(&reopened, &workspace, &policy, jan(2) + 2, force)
                    .unwrap()
                    .is_none()
            );
            let status = reopened.backup_status(Some(&workspace)).unwrap();
            assert!(
                status.retention_error.is_none(),
                "cleanup was skipped when reusing today's backup, force={force}"
            );
            assert_eq!(status.last_attempt.unwrap().id, attempt.id);
            assert_eq!(status.last_success.unwrap().id, latest.id);
            assert!(!previous.path.exists());
            assert_eq!(fs::read(&latest.path).unwrap(), latest_bytes);
            let restored = reopened
                .read_workspace_package(File::open(&latest.path).unwrap())
                .unwrap();
            assert_eq!(
                reopened
                    .page(&restored.id, &page.info.id)
                    .unwrap()
                    .info
                    .title,
                "较新快照中的页面"
            );
        }
    }

    #[test]
    fn reopening_after_publication_confirms_the_existing_package() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let workspace = store.create_workspace("中断恢复").unwrap().id;
        let policy = BackupPolicy::default();
        let attempt = run(&store, &workspace, &policy, jan(2), false)
            .unwrap()
            .unwrap();
        let path = journal_path(store.data_root(), &workspace).unwrap();
        persist_json(
            &path,
            &Journal {
                last_attempt: Some(BackupAttempt {
                    state: BackupRunState::Running,
                    finished_at: None,
                    ..attempt.clone()
                }),
                ..Journal::default()
            },
        )
        .unwrap();
        drop(store);

        let reopened = Store::open(root.path()).unwrap();
        let status = reopened.backup_status(Some(&workspace)).unwrap();
        assert_eq!(
            status.last_attempt.unwrap().state,
            BackupRunState::Interrupted
        );
        assert!(
            run(&reopened, &workspace, &policy, jan(2) + 10, false)
                .unwrap()
                .is_none()
        );
        let status = reopened.backup_status(Some(&workspace)).unwrap();
        assert_eq!(
            status.last_attempt.unwrap().state,
            BackupRunState::Completed
        );
        assert_eq!(status.last_success.unwrap().id, attempt.id);
    }

    #[cfg(windows)]
    #[test]
    fn recovered_backup_is_recorded_before_pruning_when_the_journal_is_locked() {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 1;
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let workspace = store.create_workspace("恢复后保留旧备份").unwrap().id;
        let policy = BackupPolicy {
            keep_daily: 2,
            keep_weekly: 0,
            ..BackupPolicy::default()
        };
        run(&store, &workspace, &policy, jan(1), false)
            .unwrap()
            .unwrap();
        let path = journal_path(store.data_root(), &workspace).unwrap();
        let mut interrupted: Journal = load(&path).unwrap();
        let older = interrupted.points[0].recovery.clone();

        let page = store
            .create_page(&workspace, None, "中断前已提交的页面", "file")
            .unwrap();
        let attempt = run(&store, &workspace, &policy, jan(2), false)
            .unwrap()
            .unwrap();
        let newer = store
            .backup_status(Some(&workspace))
            .unwrap()
            .last_success
            .unwrap();
        assert_eq!(newer.id, attempt.id);
        interrupted.last_attempt = Some(BackupAttempt {
            state: BackupRunState::Running,
            finished_at: None,
            ..attempt
        });
        // The new ZIP has been published while the journal still points to the old backup.
        persist_json(&path, &interrupted).unwrap();
        let old_journal = fs::read(&path).unwrap();
        let old_archive = fs::read(&older.path).unwrap();
        let new_archive = fs::read(&newer.path).unwrap();
        drop(store);

        let store = Store::open(root.path()).unwrap();
        let policy = BackupPolicy {
            keep_daily: 1,
            ..policy
        };
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        let error = run(&store, &workspace, &policy, jan(2) + 1, false).unwrap_err();
        assert_eq!(error.code, "IO");
        assert_eq!(fs::read(&path).unwrap(), old_journal);
        assert!(
            older.path.is_file(),
            "the previous backup was pruned before the journal was committed"
        );
        assert_eq!(fs::read(&older.path).unwrap(), old_archive);
        assert_eq!(fs::read(&newer.path).unwrap(), new_archive);
        drop(locked);

        assert!(
            run(&store, &workspace, &policy, jan(2) + 1, false)
                .unwrap()
                .is_none()
        );
        let saved: Journal = load(&path).unwrap();
        assert_eq!(saved.last_attempt.unwrap().state, BackupRunState::Completed);
        assert_eq!(saved.points.len(), 1);
        assert_eq!(saved.points[0].recovery.id, newer.id);
        assert!(saved.retention_error.is_none());
        assert!(!older.path.exists());
        assert_eq!(fs::read(&newer.path).unwrap(), new_archive);
        let restored = store
            .read_workspace_package(File::open(&newer.path).unwrap())
            .unwrap();
        assert_eq!(
            store.page(&restored.id, &page.info.id).unwrap().info.title,
            "中断前已提交的页面"
        );
    }
}

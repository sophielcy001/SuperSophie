// SPDX-License-Identifier: AGPL-3.0-or-later

use std::{fs::File, io};

/// Sync an export file before publication. On macOS, fall back to POSIX
/// `fsync` when the destination volume does not support the full sync.
pub fn sync_export_file(file: &File) -> io::Result<()> {
    match file.sync_all() {
        #[cfg(target_os = "macos")]
        Err(error) if error.kind() == io::ErrorKind::Unsupported => {
            rustix::fs::fsync(file).map_err(Into::into)
        }
        result => result,
    }
}

//! Private atomic state, with a nonblocking lock spanning one experiment. A crashed writer
//! leaves its pending reservation. Readers see either complete revision through atomic rename.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::BoxError;
use crate::discovery::{Discovery, MAX_BYTES};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub struct Store {
    path: PathBuf,
    _lock: File,
}

fn options() -> OpenOptions {
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut options = OpenOptions::new();
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        options
    };
    #[cfg(not(unix))]
    let options = OpenOptions::new();
    options
}

fn private(file: &File) -> Result<(), BoxError> {
    if !file.metadata()?.is_file() {
        return Err("Discovery state and lock must be regular files".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn read(path: &Path) -> Result<Discovery, BoxError> {
    let file = options().read(true).open(path)?;
    private(&file)?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_BYTES)? + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err("Discovery file exceeds 16 MiB".into());
    }
    let discovery: Discovery = serde_json::from_slice(&bytes)?;
    discovery.validate()?;
    Ok(discovery)
}

impl Store {
    pub fn lock(path: &Path) -> Result<Self, BoxError> {
        let absolute = std::path::absolute(path)?;
        let parent = absolute
            .parent()
            .ok_or("State file has no parent directory")?
            .canonicalize()?;
        let path = parent.join(absolute.file_name().ok_or("State file has no name")?);
        let mut lock_name = path.as_os_str().to_owned();
        lock_name.push(".lock");
        let file = options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_name)?;
        private(&file)?;
        file.try_lock()
            .map_err(|e| format!("Discovery is busy or cannot be locked: {e}"))?;
        Ok(Self { path, _lock: file })
    }

    pub fn load(&self) -> Result<Discovery, BoxError> {
        read(&self.path)
    }

    pub fn create(&self, value: &impl serde::Serialize) -> Result<(), BoxError> {
        self.publish(value, false)
    }

    pub fn save(&self, value: &impl serde::Serialize) -> Result<(), BoxError> {
        self.publish(value, true)
    }

    fn publish(&self, value: &impl serde::Serialize, replace: bool) -> Result<(), BoxError> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_BYTES {
            return Err("Discovery state exceeds 16 MiB".into());
        }
        let parent = self.path.parent().ok_or("State file has no parent")?;
        let temporary = parent.join(format!(
            ".discovery-{}-{}.tmp",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        // Only clean up a temporary file this invocation actually created.
        let mut file = options().write(true).create_new(true).open(&temporary)?;
        let result = (|| -> Result<(), BoxError> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            if replace {
                std::fs::rename(&temporary, &self.path)?;
            } else {
                // Unlike rename, link atomically refuses an existing destination, including
                // one created by a writer that does not participate in our lock protocol.
                std::fs::hard_link(&temporary, &self.path)?;
                std::fs::remove_file(&temporary)?;
            }
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        let _ = std::fs::remove_file(temporary);
        result
    }
}

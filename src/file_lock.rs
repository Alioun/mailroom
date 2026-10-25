use std::fs::{File, TryLockError};

pub(crate) struct ExclusiveFileLock(File);

impl ExclusiveFileLock {
    pub(crate) fn try_lock(file: File) -> Result<Self, TryLockError> {
        file.try_lock()?;
        Ok(Self(file))
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        // Closing alone leaves the lock held by descriptors inherited during process creation.
        let _ = self.0.unlock();
    }
}

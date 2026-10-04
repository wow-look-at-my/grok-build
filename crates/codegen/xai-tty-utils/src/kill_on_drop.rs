//! Kill-and-reap-on-drop ownership of a `std::process::Child`.

/// Kill-and-reap-on-drop ownership of a `std::process::Child`.
#[must_use = "KillOnDrop kills the child when dropped; bind it for the child's intended lifetime"]
pub struct KillOnDrop(std::mem::ManuallyDrop<std::process::Child>);

impl KillOnDrop {
    pub fn new(child: std::process::Child) -> Self {
        Self(std::mem::ManuallyDrop::new(child))
    }

    /// Release the child without killing it (e.g. after it was reaped).
    #[must_use = "the released child is no longer guarded; discard it only after reaping"]
    pub fn into_inner(self) -> std::process::Child {
        let mut this = std::mem::ManuallyDrop::new(self);
        // SAFETY: `this` is wrapped in ManuallyDrop.
        unsafe { std::mem::ManuallyDrop::take(&mut this.0) }
    }
}

impl std::ops::Deref for KillOnDrop {
    type Target = std::process::Child;

    fn deref(&self) -> &std::process::Child {
        &self.0
    }
}

impl std::ops::DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut std::process::Child {
        &mut self.0
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // SAFETY: drop runs at most once, and `into_inner` suppresses it via ManuallyDrop.
        let mut child = unsafe { std::mem::ManuallyDrop::take(&mut self.0) };
        let _ = child.kill();
        let _ = child.wait();
    }
}

// The fixtures spawn `sleep` and probe `/proc`.
#[cfg(all(test, unix))]
#[path = "kill_on_drop_tests.rs"]
mod tests;

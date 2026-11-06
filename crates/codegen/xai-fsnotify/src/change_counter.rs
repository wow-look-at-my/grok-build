//! A count of the events that the shared watcher for one directory sends. A
//! cache keeps the count from before its scan.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::broadcast::error::RecvError;

use crate::error::FsNotifyError;
use crate::registry::shared;
use crate::source::FsConfig;

/// Counts every [`crate::FsEvent`] for a directory. A lag also counts,
/// because it hides events.
pub struct ChangeCounter {
    count: Arc<AtomicU64>,
    live: Arc<AtomicBool>,
    task: tokio::task::AbortHandle,
}

impl ChangeCounter {
    /// Subscribes to the process-wide watcher for `dir`, so a directory that a
    /// session already watches costs no more OS watches.
    /// This call blocks until the OS watcher is ready, which can take seconds on a large tree.
    pub fn start(dir: &Path) -> Result<Self, FsNotifyError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| FsNotifyError::NoRuntime)?;
        let source = shared(dir.to_path_buf(), FsConfig::default())?;
        let mut events = source.subscribe();
        let count = Arc::new(AtomicU64::new(0));
        let live = Arc::new(AtomicBool::new(true));
        let (task_count, task_live) = (count.clone(), live.clone());
        let task = xai_spawn::spawn_on(&handle, async move {
            let _source = source;
            loop {
                match events.recv().await {
                    Ok(_) | Err(RecvError::Lagged(_)) => {
                        task_count.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(RecvError::Closed) => {
                        task_live.store(false, Ordering::Release);
                        return;
                    }
                }
            }
        });
        Ok(Self {
            count,
            live,
            task: task.abort_handle(),
        })
    }

    /// The number of events so far. `None` after the event stream closed: the count then shows nothing.
    #[must_use]
    pub fn current(&self) -> Option<u64> {
        self.live
            .load(Ordering::Acquire)
            .then(|| self.count.load(Ordering::Acquire))
    }
}

impl Drop for ChangeCounter {
    fn drop(&mut self) {
        self.task.abort();
    }
}

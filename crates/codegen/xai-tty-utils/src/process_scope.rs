//! Kill-handle for a session's child-process trees (`Send + Sync`).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use crate::{HANGUP_GRACE, ProcessGroup, new_process_group};

/// A `Send + Sync` kill-handle for one unit's child-process trees. Cheap to
/// clone (shares one inner via `Arc`).
#[derive(Clone)]
pub struct ProcessScope {
    inner: Arc<ScopeInner>,
}

struct ScopeInner {
    /// One `Weak` per enrolled child tree.
    groups: Mutex<Vec<Weak<ProcessGroup>>>,
    /// Latched once [`kill_all`](ProcessScope::kill_all) has run.
    closed: AtomicBool,
}

impl ProcessScope {
    /// Create an empty scope. Infallible — group/job handles are created lazily
    /// as children are enrolled.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ScopeInner {
                groups: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Enrollment sites use this to re-check after work done between a
    /// successful [`register`] and publishing the child.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    /// Configure `cmd` so its spawned child becomes the leader of a new
    /// process group / job.
    pub fn prepare(&self, cmd: &mut tokio::process::Command) {
        new_process_group(cmd);
    }

    /// The scope keeps only a [`Weak`]; the caller MUST keep the `Arc` alive
    /// for as long as the child is its responsibility. Returns `true` if the
    /// group was enrolled.
    pub fn register(&self, group: &Arc<ProcessGroup>) -> bool {
        let mut groups = self.lock();
        if self.inner.closed.load(Ordering::Relaxed) {
            // The scope was already reclaimed (`kill_all` ran) and won't run
            // again.
            if !group.wants_hangup() {
                let _ = group.kill();
            }
            return false;
        }
        groups.retain(|w| w.strong_count() > 0);
        groups.push(Arc::downgrade(group));
        true
    }

    /// Errors if the child already exited (nothing to attach) — not a leak — or if the scope was already closed, in which
    /// case the child has been killed and the caller must not proceed with it.
    #[must_use = "the returned Arc<ProcessGroup> must be kept alive or the scope cannot reap the child"]
    pub fn enroll(&self, child: &tokio::process::Child) -> io::Result<Arc<ProcessGroup>> {
        let mut group = ProcessGroup::new()?;
        group.attach(child)?;
        self.register_owned(group)
    }

    /// The child must be (or lead) its own group/job — spawn via
    /// [`crate::detach_std_command`] (Unix `setsid`) first, otherwise later
    /// `kill` signals a group that is not this child's.
    #[must_use = "the returned Arc<ProcessGroup> must be kept alive until the child is reaped"]
    pub fn enroll_std(&self, child: &std::process::Child) -> io::Result<Arc<ProcessGroup>> {
        let mut group = ProcessGroup::new()?;
        group.attach_std(child)?;
        self.register_owned(group)
    }

    fn register_owned(&self, group: ProcessGroup) -> io::Result<Arc<ProcessGroup>> {
        let group = Arc::new(group);
        if !self.register(&group) {
            return Err(io::Error::other(
                "process scope already closed; child killed",
            ));
        }
        Ok(group)
    }

    /// Enroll a shell this process did not spawn through a
    /// [`tokio::process::Command`]. Teardown gives it [`HANGUP_GRACE`] to hang
    /// up before the kill.
    #[must_use = "the returned Arc<ProcessGroup> must be kept alive or the scope cannot reap the child"]
    pub fn enroll_terminal_pid(&self, pid: u32) -> io::Result<Arc<ProcessGroup>> {
        let mut group = ProcessGroup::new()?;
        group.attach_pid(pid)?;
        group.hang_up_before_kill();
        let group = Arc::new(group);
        if !self.register(&group) {
            // Lost the close/spawn race; reap in the same order teardown uses.
            reap_groups(&[Arc::downgrade(&group)]);
            return Err(io::Error::other(
                "process scope already closed; child killed",
            ));
        }
        Ok(group)
    }

    /// Convenience for simple sites: [`prepare`] + spawn + [`enroll`]. Returns the child together with the owning
    /// `Arc<ProcessGroup>`, which the caller must keep alive for the scope to be able to reap the child.
    #[must_use = "the returned Arc<ProcessGroup> must be kept alive or the scope cannot reap the child"]
    pub fn spawn(
        &self,
        mut cmd: tokio::process::Command,
    ) -> io::Result<(tokio::process::Child, Arc<ProcessGroup>)> {
        self.prepare(&mut cmd);
        #[allow(clippy::disallowed_methods)]
        // ProcessScope::spawn is the enrollment primitive itself.
        let child = cmd.spawn()?;
        let group = self.enroll(&child)?;
        Ok((child, group))
    }

    /// [`spawn`] for synchronous `std::process::Command`. Detaches the child
    /// into its own group, then enrolls it. `cmd` must not already be
    /// detached or assigned a process group.
    #[must_use = "the returned Arc<ProcessGroup> must be kept alive or the scope cannot reap the child"]
    pub fn spawn_std(
        &self,
        mut cmd: std::process::Command,
    ) -> io::Result<(std::process::Child, Arc<ProcessGroup>)> {
        crate::detach_std_command(&mut cmd);
        #[allow(clippy::disallowed_methods)]
        // ProcessScope::spawn_std is the enrollment primitive itself.
        let child = cmd.spawn()?;
        let group = self.enroll_std(&child)?;
        Ok((child, group))
    }

    /// Idempotently kill every still-owned process tree (`killpg(SIGKILL)` /
    /// `TerminateJobObject`). Safe to call multiple times and from any
    /// thread.
    pub fn kill_all(&self) {
        let enrolled = {
            let mut groups = self.lock();
            let enrolled = std::mem::take(&mut *groups);
            // Latch closed under the lock: a concurrent `register` either already pushed (its group is in `enrolled` and dies below) or now sees `closed`.
            self.inner.closed.store(true, Ordering::Relaxed);
            enrolled
        };
        reap_groups(&enrolled);
    }

    /// Lock the group set, tolerating a poisoned mutex: the critical sections
    /// here are panic-free.
    #[allow(clippy::disallowed_methods)]
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Weak<ProcessGroup>>> {
        self.inner
            .groups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Number of still-live enrolled groups: weaks whose owning `Arc` has not
    /// yet been dropped.
    pub fn live_count(&self) -> usize {
        self.lock().iter().filter(|w| w.strong_count() > 0).count()
    }
}

impl Default for ProcessScope {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn sites (e.g. the local terminal backend) enroll their children here.
pub fn global_process_scope() -> &'static ProcessScope {
    static GLOBAL: OnceLock<ProcessScope> = OnceLock::new();
    GLOBAL.get_or_init(ProcessScope::new)
}

impl Drop for ScopeInner {
    fn drop(&mut self) {
        // RAII backstop: if the last scope handle drops without an explicit `kill_all`, still reap any group whose owner is alive (a wedged unit).
        #[allow(clippy::disallowed_methods)]
        let groups = self.groups.lock().unwrap_or_else(PoisonError::into_inner);
        reap_groups(&groups);
    }
}

/// Hang up the groups that asked for it, one shared grace, then kill. A failed signal means the group already exited
/// (ESRCH), which is benign and not actionable at this layer.
fn reap_groups(enrolled: &[Weak<ProcessGroup>]) {
    let mut hung_up = false;
    for group in enrolled.iter().filter_map(Weak::upgrade) {
        if group.wants_hangup() {
            let _ = group.hangup();
            hung_up = true;
        }
    }
    if hung_up {
        std::thread::sleep(HANGUP_GRACE);
    }
    // Upgrade again rather than holding the `Arc`s across the grace.
    for group in enrolled.iter().filter_map(Weak::upgrade) {
        let _ = group.kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sleeper() -> tokio::process::Command {
        let mut c = tokio::process::Command::new("sleep");
        c.arg("1000");
        c
    }

    /// `wait()` completing == the process died (and is reaped). If the kill
    /// failed, the `sleep 1000` would run on and `wait()` would time out.
    async fn died(child: &mut tokio::process::Child) -> bool {
        tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn kill_all_reaps_every_enrolled_child() {
        let scope = ProcessScope::new();
        // The owner (here, the test) keeps the Arcs alive — as a live spawn site would for as long.
        let (mut c1, _g1) = scope.spawn(sleeper()).unwrap();
        let (mut c2, _g2) = scope.spawn(sleeper()).unwrap();
        assert_eq!(scope.live_count(), 2);

        scope.kill_all();
        assert!(died(&mut c1).await, "child 1 must die after kill_all");
        assert!(died(&mut c2).await, "child 2 must die after kill_all");
    }

    #[tokio::test]
    async fn kill_all_is_idempotent() {
        let scope = ProcessScope::new();
        let (mut c, _g) = scope.spawn(sleeper()).unwrap();
        scope.kill_all();
        scope.kill_all(); // second call must not panic / error
        assert!(died(&mut c).await);
    }

    /// PID-reuse safety: once the owner drops its `Arc` (simulating a clean
    /// reap), the scope no longer references the group and `kill_all` is a no-op
    /// for it — it must NOT `killpg` a now-reapable/reused group id.
    #[tokio::test]
    async fn kill_all_skips_group_whose_owner_dropped() {
        let scope = ProcessScope::new();
        let (mut c, group) = scope.spawn(sleeper()).unwrap();
        assert_eq!(scope.live_count(), 1);

        // Owner reaps + releases ownership.
        drop(group);
        assert_eq!(
            scope.live_count(),
            0,
            "dropping the owner's Arc must make the scope's weak dead"
        );

        scope.kill_all(); // must be a no-op for the now-unowned group
        // The child was never killed by the scope; clean it up so the test doesn't leak a real `sleep` process.
        let _ = c.start_kill();
        let _ = c.wait().await;
    }

    #[tokio::test]
    async fn drop_reaps_children_while_owner_alive() {
        let scope = ProcessScope::new();
        // Owner Arc is held by the test, so the scope's weak is live.
        let (mut c, _g) = scope.spawn(sleeper()).unwrap();
        drop(scope);
        assert!(
            died(&mut c).await,
            "dropping the scope must reap a still-owned enrolled child"
        );
    }

    /// Close/spawn race: a child enrolled *after* `kill_all` must be killed on
    /// the spot, not leaked — `kill_all` won't run again to catch it — and the
    /// caller must be told (enroll errors) so it doesn't proceed with a dead child.
    #[tokio::test]
    async fn only_a_terminal_enrollment_asks_for_a_hangup() {
        let scope = ProcessScope::new();
        let mut cmd = sleeper();
        scope.prepare(&mut cmd);
        let (child, group) = scope.spawn(cmd).unwrap();
        let pid = child.id().expect("pid");
        assert!(!group.wants_hangup());

        let terminal = scope.enroll_terminal_pid(pid).unwrap();
        assert!(terminal.wants_hangup());

        scope.kill_all();
    }

    #[tokio::test]
    async fn register_after_kill_all_reaps_immediately() {
        let scope = ProcessScope::new();
        scope.kill_all(); // close the scope

        let mut cmd = sleeper();
        scope.prepare(&mut cmd);
        #[allow(clippy::disallowed_methods)] // test: exercises enroll() after close
        let mut child = cmd.spawn().unwrap();
        assert!(
            scope.enroll(&child).is_err(),
            "post-close enroll must surface the closed scope"
        );
        assert_eq!(
            scope.live_count(),
            0,
            "a post-close register must not enroll the group"
        );
        assert!(
            died(&mut child).await,
            "a child registered after kill_all must be killed immediately"
        );
    }

    fn std_sleeper() -> std::process::Command {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("1000");
        crate::detach_std_command(&mut cmd);
        cmd
    }

    #[test]
    fn enroll_std_after_close_kills_and_leaves_reap_to_caller() {
        let scope = ProcessScope::new();
        scope.kill_all();
        #[allow(clippy::disallowed_methods)] // test: exercises enroll_std after close
        let mut child = std_sleeper().spawn().expect("spawn sleeper");

        assert!(scope.enroll_std(&child).is_err());
        assert!(child.wait().expect("caller reaps child").code().is_none());
        assert_eq!(scope.live_count(), 0);
    }

    #[test]
    fn enroll_std_owner_accepts_close_after_registration() {
        let scope = ProcessScope::new();
        #[allow(clippy::disallowed_methods)] // test: exercises enroll_std
        let mut child = std_sleeper().spawn().expect("spawn sleeper");
        let group = scope.enroll_std(&child).expect("enroll child");

        scope.kill_all();
        let status = crate::wait_child_bounded(&mut child, Duration::from_secs(3))
            .expect("wait for killed child")
            .expect("killed child reaches terminal status");
        assert!(!status.success());
        drop(group);
        assert_eq!(scope.live_count(), 0);
    }
}

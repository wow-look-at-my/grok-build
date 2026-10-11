//! The coordinator's inter-agent resource lock: the registry of held keys.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use tokio::sync::oneshot;
use xai_tool_types::resource_lock::{ResourceKey, resolve_resource_conflict};

use super::coordinator_state::DisplacedCompletedChild;
use super::types::{AgentAddress, SubagentRequest, SubagentResult};

/// Configuration for the inter-agent resource lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLockConfig {
    /// Whether the spawn path enforces the lock at all.
    pub enabled: bool,
    /// Whether a colliding spawn may be moved into its own temporary worktree instead of waiting.
    pub allow_separate_worktree: bool,
    /// How long a colliding spawn waits for the holder to release before it fails outright.
    pub wait_budget: Duration,
}

impl Default for ResourceLockConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_separate_worktree: true,
            wait_budget: Duration::from_secs(30),
        }
    }
}

/// The resource key a spawn declares, if any. An explicit
/// [`xai_tool_types::SubagentResource`] wins; otherwise an explicit `cwd` is
/// the resource. A spawn that declares neither takes no lock.
pub(super) fn resource_key_for(request: &SubagentRequest) -> Option<ResourceKey> {
    if let Some(resource) = request.runtime_overrides.resource.as_ref() {
        return Some(ResourceKey::from_resource(resource));
    }
    request.cwd.as_deref().map(ResourceKey::repo_path)
}

/// The result of trying to take a resource key.
pub(super) enum LockOutcome {
    /// The caller now holds the key.
    Acquired,
    /// Another live child holds it.
    Conflict { holder: String },
}

/// One held key: the lock owner it belongs to, and the children holding it
/// for that owner.
struct Hold {
    owner: String,
    members: Vec<String>,
}

/// Which children hold which resource key, and for which lock owner. A lock
/// owner is the agent a hold belongs to.
#[derive(Default)]
pub(super) struct ResourceLockRegistry {
    holders: HashMap<ResourceKey, Hold>,
    held_by: HashMap<String, Vec<ResourceKey>>,
    /// The lock owner of every live child.
    owners: HashMap<String, String>,
    /// Holders whose worktree was created by the conflict policy.
    temporary_worktrees: HashSet<String>,
}

impl ResourceLockRegistry {
    /// The lock owner `request` holds for. A child its caller blocks on holds
    /// for the caller's owner, which is the caller itself when the caller is
    /// not a child. Any other child is its own owner.
    pub(super) fn owner_for(&self, request: &SubagentRequest) -> String {
        if !request.blocks_its_caller() {
            return request.id.clone();
        }
        self.owners
            .get(&request.parent_session_id)
            .cloned()
            .unwrap_or_else(|| request.parent_session_id.clone())
    }

    /// Record the lock owner of a child that is starting.
    pub(super) fn register(&mut self, holder: &str, owner: String) {
        self.owners.insert(holder.to_owned(), owner);
    }

    /// Whether `owner` can take `key` now.
    pub(super) fn is_free(&self, key: &ResourceKey, owner: &str) -> bool {
        self.holders.get(key).is_none_or(|hold| hold.owner == owner)
    }

    /// Take `key` for `holder` on behalf of `owner`, or report who holds it.
    pub(super) fn acquire(&mut self, key: ResourceKey, holder: &str, owner: &str) -> LockOutcome {
        match self.holders.get_mut(&key) {
            Some(hold) if hold.owner != owner => {
                return LockOutcome::Conflict {
                    holder: hold.members[0].clone(),
                };
            }
            Some(hold) => hold.members.push(holder.to_owned()),
            None => {
                self.holders.insert(
                    key.clone(),
                    Hold {
                        owner: owner.to_owned(),
                        members: vec![holder.to_owned()],
                    },
                );
            }
        }
        self.held_by.entry(holder.to_owned()).or_default().push(key);
        LockOutcome::Acquired
    }

    /// Mark `holder`'s worktree as one the conflict policy created.
    pub(super) fn mark_temporary_worktree(&mut self, holder: &str) {
        self.temporary_worktrees.insert(holder.to_owned());
    }

    /// Release every key `holder` took. A key stays held while another member
    /// still holds it. Returns whether its worktree was policy-created (and is
    /// therefore disposable).
    pub(super) fn release_all(&mut self, holder: &str) -> bool {
        self.owners.remove(holder);
        if let Some(keys) = self.held_by.remove(holder) {
            for key in keys {
                let Some(hold) = self.holders.get_mut(&key) else {
                    continue;
                };
                hold.members.retain(|member| member != holder);
                if hold.members.is_empty() {
                    self.holders.remove(&key);
                }
            }
        }
        self.temporary_worktrees.remove(holder)
    }
}

/// A spawn parked because the resource it declared is already held.
pub(super) struct LockWaiter {
    pub(super) request: Box<SubagentRequest>,
    pub(super) spawn_reply: Option<oneshot::Sender<SubagentResult>>,
    pub(super) agent_address: Option<AgentAddress>,
    pub(super) spawner_session_id: Option<String>,
    pub(super) wake_origin: Option<super::coordinator_state::WakeOrigin>,
    /// Set for a wake promotion; restored if the wait is abandoned.
    pub(super) wake: Option<DisplacedCompletedChild>,
    /// Time already spent in the admission queue before parking.
    pub(super) queued_for: Option<Duration>,
    /// The caller's foreground await deadline, carried through the wait.
    pub(super) foreground_deadline: Option<tokio::time::Instant>,
    /// When the bounded lock wait gives up and the spawn fails.
    pub(super) deadline: tokio::time::Instant,
    pub(super) key: ResourceKey,
    /// The lock owner the spawn will hold the key for.
    pub(super) owner: String,
    /// Who held the key when the spawn parked, for the failure message.
    pub(super) holder: String,
}

/// Outcome of the conflict policy for one colliding spawn.
pub(super) enum ConflictDecision {
    /// Isolate the spawn into its own temporary worktree.
    Isolate,
    /// Park the spawn until the holder releases.
    Wait,
}

/// Decide how to resolve a collision for `request`, whose key is held by
/// `holder`.
pub(super) fn decide_conflict(
    request: &SubagentRequest,
    config: &ResourceLockConfig,
) -> ConflictDecision {
    let isolation = request.runtime_overrides.isolation.unwrap_or_default();
    match resolve_resource_conflict(
        isolation,
        request.cwd.is_some(),
        config.allow_separate_worktree,
    ) {
        xai_tool_types::resource_lock::ResourceConflictResolution::SeparateWorktree => {
            ConflictDecision::Isolate
        }
        xai_tool_types::resource_lock::ResourceConflictResolution::Wait => ConflictDecision::Wait,
    }
}

/// A queue of parked lock waiters. Implemented over `VecDeque` so a waiter is
/// never lost while another is being resolved.
#[derive(Default)]
pub(super) struct LockWaitQueue {
    entries: VecDeque<LockWaiter>,
}

impl LockWaitQueue {
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(super) fn push_back(&mut self, waiter: LockWaiter) {
        self.entries.push_back(waiter);
    }

    pub(super) fn iter(&self) -> std::collections::vec_deque::Iter<'_, LockWaiter> {
        self.entries.iter()
    }

    pub(super) fn take(&mut self) -> VecDeque<LockWaiter> {
        std::mem::take(&mut self.entries)
    }

    pub(super) fn from_entries(entries: VecDeque<LockWaiter>) -> Self {
        Self { entries }
    }
}

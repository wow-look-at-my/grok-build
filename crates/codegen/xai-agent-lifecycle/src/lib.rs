//! Host-agnostic agent lifecycle hooks shared by multiple embedding crates (e.g. xai-grok-shell).

#![deny(clippy::indexing_slicing)]

pub mod local;
pub mod send;

pub use local::{
    LocalCommandContributor, LocalExtensionRegistry, LocalExtensionRegistryBuilder,
    LocalSessionLifecycleContributor, LocalTurnInputContributor, LocalTurnLifecycleContributor,
};
pub use send::{
    AnalyticsClass, CommandAction, CommandContributor, CommandInvocation, CommandSpec,
    CompactionClass, ExtensionRegistry, ExtensionRegistryBuilder, InputAuthority, InputPolicy,
    QueuePolicy, SessionIdleInput, SessionLifecycleContributor, ShutdownPolicy, SlashAuthority,
    TurnAbortInput, TurnAbortReason, TurnBoundary, TurnDoneInput, TurnErrorInput, TurnInputContext,
    TurnInputContributor, TurnInputFragment, TurnLifecycleContributor, TurnStartInput,
};

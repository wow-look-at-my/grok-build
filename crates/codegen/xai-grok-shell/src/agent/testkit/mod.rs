//! In-process e2e harness that stands up a real `MvpAgent` over ACP pipes.

pub mod e2e;

pub use crate::agent::subagent::isolated_spawn_e2e::{
    IsolatedSubagentSpawn, spawn_isolated_subagent_for_e2e,
};

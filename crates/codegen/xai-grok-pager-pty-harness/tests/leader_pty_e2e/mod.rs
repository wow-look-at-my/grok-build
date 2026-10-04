//! Leader-mode PTY e2e tests, split out of the shared `pty_e2e` target.

mod common;

mod campaign_leader_mode_remote_dismiss_on_model_pick;
mod leader_n_clients_shared_session;
mod leader_reattach_cancellation_roundtrips_durable_log;
mod leader_reattach_completion_roundtrips_durable_log;
mod leader_two_clients_shared_session;

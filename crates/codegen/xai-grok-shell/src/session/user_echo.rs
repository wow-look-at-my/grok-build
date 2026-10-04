//! Live `user_message_chunk` during a prompt is gated by `x.ai/userMessageEcho`.

/// Advertised in `initialize`'s `clientCapabilities._meta`. Absent means persist-only. `true` is live echo.
pub const USER_MESSAGE_ECHO_CAPABILITY: &str = "x.ai/userMessageEcho";

/// Per-session spelling injected by a leader into `session/new`, `session/load`, and `session/resume` `_meta`.
pub const CLIENT_USER_MESSAGE_ECHO_META: &str = "clientUserMessageEcho";

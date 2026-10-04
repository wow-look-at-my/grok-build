//! Closed enumeration of every JSON-RPC method on the wire.

use serde::{Deserialize, Serialize};

macro_rules! define_methods {
    (
        $(
            $(#[$var_attr:meta])*
            $variant:ident => $wire:literal
        ),* $(,)?
    ) => {
        /// Every JSON-RPC method understood by the computer hub.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum Method {
            $(
                $(#[$var_attr])*
                #[serde(rename = $wire)]
                $variant,
            )*
        }

        impl Method {
            /// Every `Method` variant, for exhaustive iteration in tests.
            pub const ALL: &'static [Method] = &[$(Self::$variant,)*];

            /// Wire string for this method.
            pub const fn as_wire_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)*
                }
            }

            /// Inverse of [`Self::as_wire_str`]. Returns `None` for strings that don't match any known method.
            pub fn from_wire_str(s: &str) -> Option<Self> {
                match s {
                    $($wire => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

/// Message prefix the hub uses when rejecting a request whose `method` string does not parse into [`Method`] — the shape an OLD hub produces.
pub const UNKNOWN_METHOD_MSG_PREFIX: &str = "unknown method `";

define_methods! {
    // harness → service
    SessionOpen => "session_open",
    SessionClose => "session_close",
    /// The harness is leaving the session but the workspace is untouched, exactly as if the harness connection had dropped.
    SessionDetach => "session_detach",
    SessionBindServer => "session_bind_server",
    SessionUnbindServer => "session_unbind_server",
    /// Attach this harness connection to an EXISTING session as an observer.
    SessionAttachServer => "session_attach_server",
    ToolsList => "tools.list",
    ToolsSearch => "tools.search",
    ToolCall => "tool.call",
    /// Sugar for [`Method::Hook`] with [`crate::HookEvent::Cancel`].
    ToolCancel => "tool.cancel",
    ToolNotify => "tool.notify",
    SystemNotify => "system.notify",
    SubscribeNotifications => "subscribe_notifications",
    UnsubscribeNotifications => "unsubscribe_notifications",
    Hook => "hook",
    Hello => "hello",
    HelloAck => "hello_ack",
    Ping => "ping",
    Pong => "pong",

    // tool_server → service
    ToolCallProgress => "tool_call_progress",
    ToolNotification => "tool.notification",
    /// Reply to a request/response hook, correlated back to the harness by `hook_id`.
    HookReply => "hook_reply",
    /// Notification (no `id`, no response); rejects surface only in hub metrics.
    TracesDonate => "traces.donate",
    /// Notification (no `id`, no response); rejects surface only in hub metrics.
    LogsDonate => "logs.donate",
    /// Notification (no `id`, no response); rejects surface only in hub metrics.
    MetricsDonate => "metrics.donate",
    /// A token-bound tool server presents its refreshed bearer on the live socket so the hub moves the socket's expiry deadline instead.
    AuthRefresh => "auth.refresh",

    // service → tool_server
    ToolCallRequest => "tool_call_request",

    // service → harness
    ToolsChanged => "tools_changed",
    SubscribeAck => "subscribe_ack",
    UnsubscribeAck => "unsubscribe_ack",

    // harness → service (server discovery) List available tool servers for the authenticated user.
    ServersList => "servers.list",

    // tool_server status lifecycle
    ToolServerStatus => "tool_server.status",
    ToolServerGetStatus => "tool_server.get_status",
    ToolServerEvict => "tool_server.evict",

    // ── Session lifecycle ───────────────────────────────────────────

    /// Full tool snapshot for a session (server → hub).
    Serve => "serve",
    /// Hub requests the server to start serving a session (hub → server). The server responds with its tool snapshot.
    SessionBind => "session.bind",
    /// Hub tells the server to stop serving a session (hub → server). Notification — no response expected.
    SessionUnbind => "session.unbind",

    // bot_client ↔ service (bot relay) Passthrough of an in-box gateway command.
    BotCommand => "bot.command",
    /// Short-lived noVNC descriptor. May wake a hibernated box.
    BotVncDescriptor => "bot.vncDescriptor",
    /// Live agent roster read from the box.
    BotRoster => "bot.roster",
    /// Off-box run-state read. Cold — never wakes the box.
    BotStatus => "bot.status",
    /// Off-box transcript page. Cold — never wakes the box.
    BotTranscriptOffbox => "bot.transcript.offbox",
    /// Caller-scoped weekly Grok Bot usage summary. Cold — never wakes the box.
    BotUsage => "bot.usage",
    /// Subscribe this connection to `bot.event` for the given agents.
    BotSubscribe => "bot.subscribe",
    /// Drop this connection's `bot.event` subscription for the given agents.
    BotUnsubscribe => "bot.unsubscribe",
    /// Record a conversation → agents index. Does not route by conversation.
    BotBindConversation => "bot.bindConversation",
    /// Report whether this connection has the agent on screen, so the harness can hold that agent's turn-finished push.
    BotPresence => "bot.presence",
    /// Hub → client event notification (not a client-callable verb).
    BotEvent => "bot.event",
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_wire_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_as_wire_str_from_wire_str() {
        for m in Method::ALL {
            assert_eq!(Method::from_wire_str(m.as_wire_str()), Some(*m));
        }
    }

    #[test]
    fn from_wire_str_returns_none_for_unknown() {
        assert_eq!(Method::from_wire_str("not_a_method"), None);
        assert_eq!(Method::from_wire_str(""), None);
    }

    #[test]
    fn snake_case_bot_method_aliases_are_rejected() {
        for alias in [
            "bot.vnc_descriptor",
            "bot.bind_conversation",
            "bot.link_status",
            "bot.linkStatus",
            "bot.transcript_offbox",
            "bot.ensure_box",
            "bot.ensureBox",
        ] {
            assert_eq!(Method::from_wire_str(alias), None, "{alias}");
            let err = serde_json::from_value::<Method>(serde_json::json!(alias)).unwrap_err();
            assert!(!err.to_string().is_empty(), "{alias}");
        }
    }
}

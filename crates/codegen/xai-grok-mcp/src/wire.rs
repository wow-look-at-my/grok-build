//! Single source of truth for the `x.ai/mcp/*` ACP wire strings.

/// Forward tool-invocation method (client to agent): `x.ai/mcp/call`.
pub const MCP_CALL: &str = "x.ai/mcp/call";

/// Reverse zero-IPC tool-invocation method (agent to client): `x.ai/mcp/sdk_call`.
pub const MCP_SDK_CALL: &str = "x.ai/mcp/sdk_call";

/// `session/new` `_meta` key listing in-process SDK MCP servers: `x.ai/mcp/servers`.
pub const MCP_SERVERS: &str = "x.ai/mcp/servers";

/// `initialize` `_meta` capability flag advertising in-process SDK MCP support (enables the SDK's `transport="acp"`): `x.ai/mcp/sdk`.
pub const MCP_SDK: &str = "x.ai/mcp/sdk";

/// Reverse elicitation method (agent to client): `x.ai/mcp/elicit`.
pub const MCP_ELICIT: &str = "x.ai/mcp/elicit";

/// Elicitation-complete notification (agent to client): `x.ai/mcp/elicit_complete`.
pub const MCP_ELICIT_COMPLETE: &str = "x.ai/mcp/elicit_complete";

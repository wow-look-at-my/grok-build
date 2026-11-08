//! Subdirectories of the session folder where tools save output for the model to read.

/// Full MCP results that are too large to show inline.
pub const MCP_OUTPUT_DIR: &str = "mcp";
/// Output logs of `bash` commands.
pub const TERMINAL_OUTPUT_DIR: &str = "terminal";
/// Full `web_fetch` pages that are too large to show inline.
pub const WEB_FETCH_OUTPUT_DIR: &str = "web_fetch";
/// Files that `web_fetch` downloads, such as PDFs.
pub const DOWNLOAD_OUTPUT_DIR: &str = "downloads";
/// Generated and edited images.
pub const IMAGE_OUTPUT_DIR: &str = "images";
/// Generated videos.
pub const VIDEO_OUTPUT_DIR: &str = "videos";
/// Images the user attached to a prompt.
pub const USER_ASSETS_DIR: &str = "assets";

/// Every session-folder subdirectory that holds files the model may read.
pub const TOOL_OUTPUT_DIRS: &[&str] = &[
    MCP_OUTPUT_DIR,
    TERMINAL_OUTPUT_DIR,
    WEB_FETCH_OUTPUT_DIR,
    DOWNLOAD_OUTPUT_DIR,
    IMAGE_OUTPUT_DIR,
    VIDEO_OUTPUT_DIR,
    USER_ASSETS_DIR,
];

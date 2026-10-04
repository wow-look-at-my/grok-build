//! Serialize local repository changes (local commits and uncommitted worktree/index changes).
pub use xai_file_utils::BlobCompression;
pub use xai_file_utils::{
    ARCHIVE_SCHEMA_VERSION, ARCHIVE_SCHEMA_VERSION_V3, DEDUP_BLOB_SUBDIR, DEDUP_GCS_PREFIX,
    DEDUP_PATCH_SUBDIR, DedupMetadata, ExcludedContent, FileReference, PatchReference,
    SKIP_DIR_NAMES, TraceExportConfig, UploadMethod, skip_dir_set,
};

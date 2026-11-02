//! macOS sandbox implementation using Seatbelt
//!
//! This is a pure sandboxing primitive - it applies ONLY the capabilities provided.
//! The caller is responsible for:
//! - Adding system paths (e.g., /usr, /lib, /System/Library) if executables need to run
//! - Implementing any security policy (sensitive path blocking, etc.)

use crate::capability::{AccessMode, CapabilitySet, NetworkMode};
use crate::error::{NonoError, Result};
use crate::sandbox::SupportInfo;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;
use tracing::{debug, info};

// FFI bindings to macOS sandbox API
// These are private APIs but have been stable for years
// Reference: https://reverse.put.as/wp-content/uploads/2011/09/Apple-Sandbox-Guide-v1.0.pdf

extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> i32;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

// FFI bindings for sandbox extension API (runtime capability expansion)
// These are documented in <sandbox.h> and stable across macOS versions.
// Extensions allow an unsandboxed supervisor to issue tokens that expand
// a sandboxed process's access for specific paths.
extern "C" {
    fn sandbox_extension_issue_file(
        extension_class: *const c_char,
        path: *const c_char,
        flags: u32,
    ) -> *mut c_char;

    fn sandbox_extension_consume(token: *const c_char) -> i64;

    fn sandbox_extension_release(handle: i64) -> i32;
}

/// Extension class for read-only access
const EXT_CLASS_READ: &str = "com.apple.app-sandbox.read";

/// Extension class for read+write access
const EXT_CLASS_READ_WRITE: &str = "com.apple.app-sandbox.read-write";

/// Issue a sandbox extension token for a path.
///
/// Called by the unsandboxed supervisor to create a token that a sandboxed
/// process can consume to expand its access for the given path.
///
/// The token is HMAC-SHA256 authenticated with a per-boot kernel key and
/// cannot be forged. It is path-specific and class-specific.
///
/// # Arguments
/// * `path` - The filesystem path to grant access to
/// * `access` - The access mode (Read -> read-only token, Write/ReadWrite -> read-write token)
///
/// # Errors
/// Returns an error if the path contains null bytes or if the kernel rejects the request.
pub fn extension_issue_file(path: &Path, access: AccessMode) -> Result<String> {
    let class = match access {
        AccessMode::Read => EXT_CLASS_READ,
        AccessMode::Write | AccessMode::ReadWrite => EXT_CLASS_READ_WRITE,
    };

    let class_c = CString::new(class)
        .map_err(|_| NonoError::SandboxInit("Extension class contains null byte".to_string()))?;

    let path_str = path.to_str().ok_or_else(|| {
        NonoError::SandboxInit(format!("Path contains non-UTF-8 bytes: {}", path.display()))
    })?;

    let path_c = CString::new(path_str).map_err(|_| {
        NonoError::SandboxInit(format!("Path contains null byte: {}", path.display()))
    })?;

    // SAFETY: sandbox_extension_issue_file takes valid C strings for class and path,
    // and a flags argument (0 for default behavior). Returns a heap-allocated C string
    // token on success, or NULL on failure. The returned string must be freed with free().
    let token_ptr = unsafe { sandbox_extension_issue_file(class_c.as_ptr(), path_c.as_ptr(), 0) };

    if token_ptr.is_null() {
        return Err(NonoError::SandboxInit(format!(
            "sandbox_extension_issue_file failed for path: {}",
            path.display()
        )));
    }

    // SAFETY: token_ptr is a valid, non-null C string returned by sandbox_extension_issue_file.
    let token = unsafe { CStr::from_ptr(token_ptr) }
        .to_string_lossy()
        .into_owned();

    // SAFETY: token_ptr was allocated by sandbox_extension_issue_file and must be freed.
    unsafe { libc::free(token_ptr.cast::<libc::c_void>()) };

    debug!(
        "Issued extension token for {} ({:?})",
        path.display(),
        access
    );
    Ok(token)
}

/// Consume a sandbox extension token to expand the current process's sandbox.
///
/// Called inside a sandboxed process (typically by the DYLD shim) to activate
/// a token received from the supervisor. After consumption, the sandbox allows
/// access to the token's path with the token's access class.
///
/// Consumed extensions survive `fork()` and `exec()` -- all child processes
/// inherit the expanded access.
///
/// Returns a handle that can be passed to [`extension_release`] to revoke the grant.
///
/// # Errors
/// Returns an error if the token is invalid, expired, or if consumption fails.
pub fn extension_consume(token: &str) -> Result<i64> {
    let token_c = CString::new(token)
        .map_err(|_| NonoError::SandboxInit("Extension token contains null byte".to_string()))?;

    // SAFETY: sandbox_extension_consume takes a valid C string token.
    // Returns a non-negative handle on success, or -1 on failure.
    let handle = unsafe { sandbox_extension_consume(token_c.as_ptr()) };

    if handle < 0 {
        return Err(NonoError::SandboxInit(format!(
            "sandbox_extension_consume failed (handle={})",
            handle
        )));
    }

    debug!("Consumed extension token (handle={})", handle);
    Ok(handle)
}

/// Release a consumed sandbox extension, revoking the dynamically-granted access.
///
/// # Errors
/// Returns an error if the handle is invalid or if the release fails.
pub fn extension_release(handle: i64) -> Result<()> {
    // SAFETY: sandbox_extension_release takes a handle from sandbox_extension_consume.
    // Returns 0 on success, -1 on failure.
    let result = unsafe { sandbox_extension_release(handle) };

    if result != 0 {
        return Err(NonoError::SandboxInit(format!(
            "sandbox_extension_release failed for handle {}",
            handle
        )));
    }

    debug!("Released extension (handle={})", handle);
    Ok(())
}

/// Check if Seatbelt sandboxing is supported
pub fn is_supported() -> bool {
    // Seatbelt is available on all modern macOS versions
    true
}

/// Get information about sandbox support
pub fn support_info() -> SupportInfo {
    SupportInfo {
        is_supported: true,
        platform: "macos",
        details: "macOS Seatbelt sandbox available".to_string(),
    }
}

/// Collect parent directories that need metadata access for path resolution.
///
/// Programs need to lstat() each path component when resolving paths.
/// For example, to access /Users/luke/.claude, Node.js needs to lstat:
/// - /Users
/// - /Users/luke
///
/// This function returns those parent directories so we can allow metadata
/// (but not data) access to them.
fn collect_parent_dirs(caps: &CapabilitySet) -> std::collections::HashSet<String> {
    let mut parents = std::collections::HashSet::new();

    for cap in caps.fs_capabilities() {
        // Collect parents for both resolved and original paths.
        // On macOS, /tmp is a symlink to /private/tmp. If the user passes
        // --allow /tmp, we need metadata access to / for the symlink itself.
        // The original path's parents handle this.
        let paths_to_walk: Vec<&std::path::Path> = if cap.original != cap.resolved {
            vec![cap.resolved.as_path(), cap.original.as_path()]
        } else {
            vec![cap.resolved.as_path()]
        };

        for path in paths_to_walk {
            let mut current = path.parent();
            while let Some(parent) = current {
                let parent_str = parent.to_string_lossy().to_string();

                // Stop at root
                if parent_str == "/" || parent_str.is_empty() {
                    break;
                }

                // If already present, ancestors were processed too - early exit
                if !parents.insert(parent_str) {
                    break;
                }
                current = parent.parent();
            }
        }
    }

    parents
}

/// Build Seatbelt path filters for a capability.
///
/// On macOS, symlinks like `/tmp` -> `/private/tmp` mean the user's original
/// path may differ from the canonicalized resolved path. Seatbelt operates on
/// literal paths, not resolved symlinks, so we must emit rules for both.
/// Returns one or two `(subpath "...")` or `(literal "...")` strings.
fn path_filters_for_cap(cap: &crate::capability::FsCapability) -> Result<Vec<String>> {
    let mut filters = Vec::with_capacity(2);

    let resolved_str = cap.resolved.to_str().ok_or_else(|| {
        NonoError::SandboxInit(format!(
            "path contains non-UTF-8 bytes: {}",
            cap.resolved.display()
        ))
    })?;
    let escaped_resolved = escape_path(resolved_str)?;
    let kind = if cap.is_file { "literal" } else { "subpath" };
    filters.push(format!("{} \"{}\"", kind, escaped_resolved));

    // If the original path differs (e.g. /tmp vs /private/tmp), emit a rule
    // for the original too so Seatbelt allows traversing the symlink.
    if cap.original != cap.resolved {
        if let Some(original_str) = cap.original.to_str() {
            let escaped_original = escape_path(original_str)?;
            filters.push(format!("{} \"{}\"", kind, escaped_original));
        }
    }

    Ok(filters)
}

/// Returns true if the capability set explicitly grants access to a keychain DB.
///
/// This is a narrow opt-in for tools that need OAuth/session refresh via macOS Keychain.
fn has_explicit_keychain_db_access(caps: &CapabilitySet) -> bool {
    let user_keychain_dbs = std::env::var("HOME").ok().map(|home| {
        [
            Path::new(&home).join("Library/Keychains/login.keychain-db"),
            Path::new(&home).join("Library/Keychains/metadata.keychain-db"),
        ]
    });
    let system_keychain_dbs = [
        Path::new("/Library/Keychains/login.keychain-db").to_path_buf(),
        Path::new("/Library/Keychains/metadata.keychain-db").to_path_buf(),
    ];

    let is_keychain_db = |path: &Path| -> bool {
        if system_keychain_dbs
            .iter()
            .any(|candidate| path == candidate)
        {
            return true;
        }
        if let Some(ref user_keychain_dbs) = user_keychain_dbs {
            if user_keychain_dbs.iter().any(|candidate| path == candidate) {
                return true;
            }
        }
        false
    };

    caps.fs_capabilities()
        .iter()
        .any(|cap| is_keychain_db(&cap.original) || is_keychain_db(&cap.resolved))
}

/// Escape a path for use in Seatbelt profile strings.
///
/// Paths are placed inside double-quoted S-expression strings where `\` and `"`
/// are the significant characters. Control characters (0x00-0x1F, 0x7F) are
/// rejected because silently stripping them would cause the sandbox rule to
/// target a different path than intended.
fn escape_path(path: &str) -> Result<String> {
    let mut result = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            c if c.is_control() => {
                return Err(NonoError::SandboxInit(format!(
                    "path contains control character 0x{:02X}: {}",
                    c as u32, path
                )));
            }
            _ => result.push(c),
        }
    }
    Ok(result)
}

/// Escape a filesystem path for use inside a Seatbelt `regex` clause.
///
/// Used for non-recursive directory socket grants: a directory `/d` is
/// emitted as `(regex "^<escaped>/[^/]+$")` so only direct-child sockets
/// are matched.
///
/// Escaping strategy, by character:
///
/// | Char | Rendered Scheme source | Regex engine sees | Matches |
/// |------|------------------------|-------------------|---------|
/// | `.`, `+`, `*`, `?`, `(`, `)`, `{`, `}`, `|`, `^`, `$` | `[.]` etc. | char class | literal `.` etc. |
/// | `[`  | `\\[`  | `\[`  | literal `[` |
/// | `]`  | `\\]`  | `\]`  | literal `]` |
/// | `\`  | `\\\\` | `\\`  | literal `\` |
/// | `"`  | `\"`   | `"`   | literal `"` |
/// | other | verbatim | verbatim | as written |
///
/// The character-class form (`[.]`) is unambiguous across both raw and
/// cooked Scheme string readers (the §13 spike verified this for `.`).
/// For the three chars that can't appear literally inside `[ ]` (`[`,
/// `]`, `\`), we fall back to Scheme-level backslash escaping: the
/// Scheme reader collapses `\\` → `\`, and the regex engine then
/// interprets `\<char>` as a literal. Backslash itself needs a second
/// layer of escaping (`\\\\` → `\\` → literal `\`).
///
/// Control characters are rejected outright, matching `escape_path`'s
/// policy.
fn regex_escape_path_for_seatbelt(path: &str) -> Result<String> {
    for c in path.chars() {
        if c.is_control() {
            return Err(NonoError::SandboxInit(format!(
                "path contains control character 0x{:02X}: {}",
                c as u32, path
            )));
        }
    }
    let mut result = String::with_capacity(path.len() * 2);
    for c in path.chars() {
        match c {
            '.' | '+' | '*' | '?' | '(' | ')' | '{' | '}' | '|' | '^' | '$' => {
                result.push('[');
                result.push(c);
                result.push(']');
            }
            // `[`, `]`: can't appear in a character class literal. Emit
            // `\\<char>` (3 source chars); Scheme reduces to `\<char>`
            // which is a regex-literal match.
            '[' | ']' => {
                result.push('\\');
                result.push('\\');
                result.push(c);
            }
            // `\`: needs double escaping — emit `\\\\` (4 source chars).
            // Scheme reduces to `\\`, which the regex engine interprets
            // as a literal backslash.
            '\\' => result.push_str("\\\\\\\\"),
            // `"`: Scheme string escape only; regex engine sees the
            // bare quote as a literal character.
            '"' => result.push_str("\\\""),
            _ => result.push(c),
        }
    }
    Ok(result)
}

/// Emit Seatbelt `(allow network-outbound …)` and `(allow network-bind …)`
/// rules for each [`UnixSocketCapability`] in `caps`.
///
/// On macOS, `connect(2)` and `bind(2)` are classified as **separate**
/// Seatbelt operations (`network-outbound` and `network-bind`), so each
/// needs its own allow-rule. Mode enforcement:
///
/// - `Connect` mode: emit `network-outbound` only. `bind(2)` stays
///   denied by the base `(deny network*)` clause.
/// - `ConnectBind` mode: emit **both** `network-outbound` and
///   `network-bind`.
///
/// Path form:
///
/// - File-scoped grants emit `(… (path "…"))`.
/// - Directory-scoped grants emit `(… (regex "^<dir>/[^/]+$"))` —
///   non-recursive (direct children only), per §5 / §13 spike.
///
/// For both forms, the `original` path is emitted too when it differs
/// from `resolved` (`/tmp` vs `/private/tmp`), preserving the dual-path
/// symlink pattern.
///
/// `FsCapability` entries intentionally **do not** trigger either rule.
/// Filesystem grants and unix-socket grants are orthogonal layers per
/// #696; the CLI-side sugar in `nono-cli` auto-registers the fs grant
/// that `bind(2)`/`connect(2)` need.
fn emit_unix_socket_rules(profile: &mut String, caps: &CapabilitySet) -> Result<()> {
    for cap in caps.unix_socket_capabilities() {
        let resolved_str = cap.resolved.to_str().ok_or_else(|| {
            NonoError::SandboxInit(format!(
                "unix socket path contains non-UTF-8 bytes: {}",
                cap.resolved.display()
            ))
        })?;
        let original_str = if cap.original != cap.resolved {
            cap.original.to_str()
        } else {
            None
        };

        // Always emit for connect (network-outbound). Bind (network-bind)
        // is only emitted for ConnectBind — Connect-only grants intentionally
        // leave `bind(2)` denied by the base `(deny network*)` clause.
        let operations: &[&str] = if cap.mode.permits_bind() {
            &["network-outbound", "network-bind"]
        } else {
            &["network-outbound"]
        };

        if cap.is_directory {
            let escaped = regex_escape_path_for_seatbelt(resolved_str)?;
            let escaped_orig = original_str
                .map(regex_escape_path_for_seatbelt)
                .transpose()?;
            for op in operations {
                profile.push_str(&format!("(allow {} (regex \"^{}/[^/]+$\"))\n", op, escaped));
                if let Some(ref e) = escaped_orig {
                    profile.push_str(&format!("(allow {} (regex \"^{}/[^/]+$\"))\n", op, e));
                }
            }
        } else {
            let escaped = escape_path(resolved_str)?;
            let escaped_orig = original_str.map(escape_path).transpose()?;
            for op in operations {
                profile.push_str(&format!("(allow {} (path \"{}\"))\n", op, escaped));
                if let Some(ref e) = escaped_orig {
                    profile.push_str(&format!("(allow {} (path \"{}\"))\n", op, e));
                }
            }
        }
    }
    Ok(())
}

/// Generate a Seatbelt profile from capabilities
///
/// This is a pure primitive - it generates rules ONLY for paths in the CapabilitySet.
/// The caller must include all necessary paths (system paths, temp dirs, etc.).
///
/// Returns an error if any path contains non-UTF-8 bytes (which would produce
/// incorrect Seatbelt rules via lossy conversion).
fn generate_profile(caps: &CapabilitySet) -> Result<String> {
    let mut profile = String::new();

    // Profile version
    profile.push_str("(version 1)\n");

    // Start with deny default
    profile.push_str("(deny default)\n");
    if caps.seatbelt_debug_deny() {
        profile.push_str("(debug deny)\n");
    }

    // Allow specific process operations needed for execution
    profile.push_str("(allow process-exec*)\n");
    profile.push_str("(allow process-fork)\n");

    // Process info: allow self-inspection and same-sandbox inspection for both
    // Isolated and AllowSameSandbox, matching Linux behaviour where Landlock
    // cannot distinguish the two. Denying process-info for same-sandbox children
    // would break health checks via proc_pidinfo() / sysctl(KERN_PROC) that
    // Node.js modules use to monitor child process state.
    //
    // We emit (target self) alongside (target same-sandbox) because Seatbelt's
    // same-sandbox filter may not subsume self — being explicit ensures the
    // process can always inspect itself regardless of implementation details.
    match caps.process_info_mode() {
        crate::capability::ProcessInfoMode::Isolated
        | crate::capability::ProcessInfoMode::AllowSameSandbox => {
            profile.push_str("(allow process-info* (target self))\n");
            profile.push_str("(allow process-info* (target same-sandbox))\n");
        }
        crate::capability::ProcessInfoMode::AllowAll => {
            profile.push_str("(allow process-info*)\n");
        }
    }

    // Allow specific system operations
    profile.push_str("(allow sysctl-read)\n");

    // Mach IPC: allow service resolution. Deny Keychain/security services by default.
    // If a keychain DB is explicitly granted, skip these denies so profiles that
    // intentionally rely on macOS Keychain OAuth refresh can work.
    //
    // Without these denies, blanket mach-lookup can permit Keychain retrieval via
    // Mach IPC, bypassing file-level deny rules in profiles that do NOT opt in.
    profile.push_str("(allow mach-lookup)\n");
    if !has_explicit_keychain_db_access(caps) {
        // Legacy keychain daemon names (macOS < 13)
        profile.push_str("(deny mach-lookup (global-name \"com.apple.SecurityServer\"))\n");
        profile.push_str("(deny mach-lookup (global-name \"com.apple.securityd\"))\n");
        // Modern keychain daemon (macOS 13 Ventura+). Legacy SecKeychain APIs
        // route here on Ventura and later, bypassing the legacy service denies above.
        // Without this deny, FFI/ctypes callers can read keychain entries despite
        // the file-level deny on ~/Library/Keychains.
        profile.push_str("(deny mach-lookup (global-name \"com.apple.security.keychaind\"))\n");
        // Modern security daemon (macOS 10.10+). SecItem APIs ("Data Protection"
        // keychain) route through secd. Blocking this prevents access to iCloud
        // Keychain and modern keychain items that bypass the legacy daemon paths.
        profile.push_str("(deny mach-lookup (global-name \"com.apple.secd\"))\n");
        // Security agent: shows keychain authorization dialogs. Without this deny, the
        // agent can act as a proxy — presenting a user prompt and returning the credential
        // on behalf of the sandboxed process even when the direct daemon paths are blocked.
        profile.push_str("(deny mach-lookup (global-name \"com.apple.security.agent\"))\n");
    }
    profile.push_str("(allow mach-per-user-lookup)\n");
    profile.push_str("(allow mach-task-name)\n");
    profile.push_str("(deny mach-priv*)\n");

    // IPC: always allow POSIX shared memory operations
    profile.push_str("(allow ipc-posix-shm-read-data)\n");
    profile.push_str("(allow ipc-posix-shm-write-data)\n");
    profile.push_str("(allow ipc-posix-shm-write-create)\n");

    // IPC: conditionally allow all POSIX semaphore operations (IpcMode::Full).
    // Required by multiprocessing runtimes (Python multiprocessing, joblib, etc.)
    // that use sem_open/sem_wait/sem_post/sem_close for worker coordination.
    // We use the wildcard to cover all sem operations (open, close, create,
    // post, wait, unlink) since Seatbelt's internal operation taxonomy is
    // not fully documented and individual enumeration risks missing operations.
    if caps.ipc_mode() == crate::capability::IpcMode::Full {
        profile.push_str("(allow ipc-posix-sem*)\n");
    }

    // Signal isolation: both Isolated and AllowSameSandbox emit
    // (target self) + (target same-sandbox). This matches Linux behaviour
    // where Landlock's LANDLOCK_SCOPE_SIGNAL scopes to the sandbox domain,
    // not to the calling process alone — making Isolated and AllowSameSandbox
    // equivalent.
    //
    // Emitting only (target self) for Isolated would prevent the sandboxed
    // process from signaling its own forked children, causing orphan process
    // accumulation when the parent calls kill(child_pid, SIGTERM) and gets
    // EPERM. Terminal-generated signals (Ctrl+C → SIGINT) bypass Seatbelt
    // since they are delivered by the kernel to the foreground process group.
    //
    // We emit both (target self) and (target same-sandbox) because Seatbelt's
    // same-sandbox filter may not subsume self — being explicit ensures the
    // process can always signal itself regardless of implementation details.
    match caps.signal_mode() {
        crate::capability::SignalMode::Isolated
        | crate::capability::SignalMode::AllowSameSandbox => {
            profile.push_str("(allow signal (target self))\n");
            profile.push_str("(allow signal (target same-sandbox))\n");
        }
        crate::capability::SignalMode::AllowAll => {
            profile.push_str("(allow signal)\n");
        }
    }
    // system-socket is NOT granted globally — each NetworkMode branch emits
    // only the socket domains it needs (AF_UNIX for DNS, AF_INET/AF_INET6
    // for TCP). AllowAll emits the blanket rule. This prevents restricted
    // modes from creating arbitrary socket types.
    profile.push_str("(allow system-fsctl)\n");
    profile.push_str("(allow system-info)\n");

    // Allow reading the root directory entry itself (required for exec path resolution)
    profile.push_str("(allow file-read* (literal \"/\"))\n");

    // Allow metadata access to parent directories of granted paths (for path resolution)
    let parent_dirs = collect_parent_dirs(caps);
    for parent in &parent_dirs {
        let escaped = escape_path(parent)?;
        profile.push_str(&format!(
            "(allow file-read-metadata (literal \"{}\"))\n",
            escaped
        ));
    }

    // Allow mapping executables into memory, restricted to readable paths.
    // This prevents loading arbitrary shared libraries via DYLD_INSERT_LIBRARIES
    // from paths outside the sandbox's read set.
    for cap in caps.fs_capabilities() {
        if matches!(cap.access, AccessMode::Read | AccessMode::ReadWrite) {
            for filter in path_filters_for_cap(cap)? {
                profile.push_str(&format!("(allow file-map-executable ({}))\n", filter));
            }
        }
    }

    // Allow file ioctl restricted to TTY/PTY devices and granted paths
    profile.push_str("(allow file-ioctl (literal \"/dev/tty\"))\n");
    profile.push_str("(allow file-ioctl (regex #\"^/dev/ttys[0-9]+$\"))\n");
    profile.push_str("(allow file-ioctl (regex #\"^/dev/pty[a-z][0-9a-f]+$\"))\n");
    // Also allow ioctl on explicitly granted paths (for interactive programs)
    for cap in caps.fs_capabilities() {
        for filter in path_filters_for_cap(cap)? {
            profile.push_str(&format!("(allow file-ioctl ({}))\n", filter));
        }
    }

    // Allow pseudo-terminal operations
    profile.push_str("(allow pseudo-tty)\n");

    // Add read rules for all capabilities with Read or ReadWrite access.
    // Emits rules for both original and resolved paths when they differ
    // (e.g. /tmp vs /private/tmp) so Seatbelt allows traversing symlinks.
    for cap in caps.fs_capabilities() {
        match cap.access {
            AccessMode::Read | AccessMode::ReadWrite => {
                for filter in path_filters_for_cap(cap)? {
                    profile.push_str(&format!("(allow file-read* ({}))\n", filter));
                }
            }
            AccessMode::Write => {
                // Write-only doesn't need read access
            }
        }
    }

    // Extension filter rules for runtime capability expansion via supervisor.
    // These allow sandbox_extension_consume() tokens to dynamically expand access.
    // The rules are inert unless a matching token is consumed -- they add no access
    // by themselves. The supervisor checks protected roots and deny groups before issuing
    // tokens, so the pre-issuance check is the enforcement point.
    if caps.extensions_enabled() {
        profile.push_str("(allow file-read* (extension \"com.apple.app-sandbox.read\"))\n");
        profile.push_str("(allow file-read* (extension \"com.apple.app-sandbox.read-write\"))\n");
        profile.push_str("(allow file-write* (extension \"com.apple.app-sandbox.read-write\"))\n");
    }

    // SECURITY: Platform deny rules are placed BETWEEN read and write rules.
    // This matches the research CLI pattern where sensitive path denials come
    // after read allows but before write allows. In Seatbelt, more specific rules
    // always win regardless of order; for equal specificity, last-match wins.
    // Placing deny rules here ensures they override read allows when equally specific,
    // while write allows below can still override deny-unlink for user-granted paths.
    for rule in caps.platform_rules() {
        profile.push_str(rule);
        profile.push('\n');
    }

    // Add write rules for all capabilities with Write or ReadWrite access.
    // These come AFTER platform deny rules so user-granted write paths can
    // override global denials like (deny file-write-unlink).
    // Emits rules for both original and resolved paths when they differ.
    for cap in caps.fs_capabilities() {
        match cap.access {
            AccessMode::Write | AccessMode::ReadWrite => {
                for filter in path_filters_for_cap(cap)? {
                    profile.push_str(&format!("(allow file-write* ({}))\n", filter));
                }
            }
            AccessMode::Read => {
                // Read-only doesn't need write access
            }
        }
    }

    // Network rules
    //
    // DNS resolution rules for restricted modes (Blocked/ProxyOnly):
    // macOS resolves all DNS through /var/run/mDNSResponder (a Unix domain
    // socket). Seatbelt classifies connect(2) on Unix sockets as
    // network-outbound, so (deny network*) blocks DNS. These rules allow
    // AF_UNIX socket creation and outbound to the mDNSResponder path (both
    // /var/run and /private/var/run since /var is a symlink on macOS).
    const MDNS_RULES: &str = "\
(allow system-socket (socket-domain AF_UNIX) (socket-type SOCK_STREAM))\n\
(allow network-outbound (path \"/private/var/run/mDNSResponder\"))\n\
(allow network-outbound (path \"/var/run/mDNSResponder\"))\n";

    let localhost_ports = caps.localhost_ports();
    match caps.network_mode() {
        NetworkMode::Blocked => {
            profile.push_str("(deny network*)\n");
            profile.push_str(MDNS_RULES);
            // Unix socket grants (see #685 / #696). Only explicit
            // UnixSocketCapability entries emit network-outbound rules;
            // generic FsCapability grants no longer implicitly grant
            // `connect()`/`bind()` to sockets inside them. Directory
            // grants use a non-recursive regex.
            emit_unix_socket_rules(&mut profile, caps)?;
            if !localhost_ports.is_empty() {
                // Allow system-socket for TCP (required for connect/bind)
                profile.push_str(
                    "(allow system-socket (socket-domain AF_INET) (socket-type SOCK_STREAM))\n",
                );
                profile.push_str(
                    "(allow system-socket (socket-domain AF_INET6) (socket-type SOCK_STREAM))\n",
                );
                for lp in localhost_ports {
                    profile.push_str(&format!(
                        "(allow network-outbound (remote tcp \"localhost:{}\"))\n",
                        lp
                    ));
                }
                // Seatbelt cannot filter bind/inbound by port
                profile.push_str("(allow network-bind)\n");
                profile.push_str("(allow network-inbound)\n");
            }
        }
        NetworkMode::ProxyOnly { port, bind_ports } => {
            // Block all network, then allow only localhost TCP to the proxy port.
            profile.push_str("(deny network*)\n");
            profile.push_str(MDNS_RULES);
            // Unix socket grants (see Blocked branch above).
            emit_unix_socket_rules(&mut profile, caps)?;
            profile.push_str(&format!(
                "(allow network-outbound (remote tcp \"localhost:{}\"))\n",
                port
            ));
            for lp in localhost_ports {
                profile.push_str(&format!(
                    "(allow network-outbound (remote tcp \"localhost:{}\"))\n",
                    lp
                ));
            }
            // Scope system-socket for TCP (required for connect/bind to proxy).
            profile.push_str(
                "(allow system-socket (socket-domain AF_INET) (socket-type SOCK_STREAM))\n",
            );
            profile.push_str(
                "(allow system-socket (socket-domain AF_INET6) (socket-type SOCK_STREAM))\n",
            );
            // If bind ports or localhost IPC ports are specified, allow network-bind
            // and network-inbound. Seatbelt cannot filter bind/inbound by port,
            // so this is a blanket allow.
            if !bind_ports.is_empty() || !localhost_ports.is_empty() {
                profile.push_str("(allow network-bind)\n");
                profile.push_str("(allow network-inbound)\n");
            }
        }
        NetworkMode::AllowAll => {
            profile.push_str("(allow system-socket)\n");
            profile.push_str("(allow network-outbound)\n");
            profile.push_str("(allow network-inbound)\n");
            profile.push_str("(allow network-bind)\n");
        }
    }

    // Per-port TCP rules are not supported on macOS (Seatbelt cannot filter by port alone).
    // ProxyOnly mode IS supported via `(remote tcp "localhost:PORT")`.
    if !caps.tcp_connect_ports().is_empty() || !caps.tcp_bind_ports().is_empty() {
        return Err(NonoError::NetworkFilterUnsupported {
            platform: "macOS".to_string(),
            reason: "Seatbelt cannot filter by TCP port. Use --allow-domain for host-level \
                     filtering (routed through the proxy) or ProxyOnly mode instead."
                .to_string(),
        });
    }

    Ok(profile)
}

/// Apply Seatbelt sandbox with the given capabilities
///
/// This is a pure primitive - it applies ONLY the capabilities provided.
/// The caller is responsible for including all necessary paths.
pub fn apply(caps: &CapabilitySet) -> Result<()> {
    let profile = generate_profile(caps)?;

    debug!("Generated Seatbelt profile:\n{}", profile);

    let profile_cstr = CString::new(profile)
        .map_err(|e| NonoError::SandboxInit(format!("Invalid profile string: {}", e)))?;

    let mut error_buf: *mut c_char = ptr::null_mut();

    // SAFETY: sandbox_init is a stable macOS API. We pass:
    // - A valid null-terminated C string for the profile
    // - 0 for raw profile mode (not a named profile)
    // - A pointer to receive any error message
    let result = unsafe {
        sandbox_init(
            profile_cstr.as_ptr(),
            0, // Raw profile mode
            &mut error_buf,
        )
    };

    if result != 0 {
        let error_msg = if !error_buf.is_null() {
            // SAFETY: sandbox_init sets error_buf to a valid C string on error
            let msg = unsafe {
                std::ffi::CStr::from_ptr(error_buf)
                    .to_string_lossy()
                    .into_owned()
            };
            // SAFETY: sandbox_free_error expects a pointer from sandbox_init
            unsafe { sandbox_free_error(error_buf) };
            msg
        } else {
            format!("sandbox_init returned error code {}", result)
        };

        return Err(NonoError::SandboxInit(error_msg));
    }

    info!("Seatbelt sandbox applied successfully");
    Ok(())
}

//! [`RetryPolicy`] — maps a non-2xx HTTP status code to a [`Disposition`].

/// What a caller should do with a non-2xx HTTP response, by status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Retryable,
    AuthRefresh,
    Terminal,
}

/// Maps an HTTP status code to a [`Disposition`].
pub struct RetryPolicy {
    retryable: &'static [u16],
    auth_refresh: &'static [u16],
    terminal: &'static [u16],
    default: Disposition,
}

impl RetryPolicy {
    /// Classify `status`. Returns `None` for 2xx (success, not an error).
    /// Any 5xx not explicitly listed as `terminal`/`auth_refresh` is
    /// `Retryable` regardless of `default`.
    pub fn classify(&self, status: u16) -> Option<Disposition> {
        if (200..300).contains(&status) {
            return None;
        }
        if self.auth_refresh.contains(&status) {
            return Some(Disposition::AuthRefresh);
        }
        if self.terminal.contains(&status) {
            return Some(Disposition::Terminal);
        }
        if self.retryable.contains(&status) || (500..600).contains(&status) {
            return Some(Disposition::Retryable);
        }
        Some(self.default)
    }

    /// `true` iff `status` classifies as `Retryable`.
    pub fn should_retry(&self, status: u16) -> bool {
        matches!(self.classify(status), Some(Disposition::Retryable))
    }

    /// This is the rule behind CCP's `x-should-retry` header.
    pub const fn server() -> Self {
        Self {
            retryable: &[429],
            auth_refresh: &[],
            terminal: &[],
            default: Disposition::Terminal,
        }
    }

    pub const fn edge_client() -> Self {
        Self {
            retryable: &[429],
            auth_refresh: &[],
            terminal: &[525, 526],
            default: Disposition::Terminal,
        }
    }

    pub const fn client_storage() -> Self {
        Self {
            retryable: &[],
            auth_refresh: &[401],
            terminal: &[400, 403, 404, 525, 526],
            default: Disposition::Retryable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_should_retry() {
        let policy = RetryPolicy::server();
        for code in [429, 500, 502, 503, 504, 501, 520] {
            assert!(policy.should_retry(code), "expected {code} to retry");
        }
        for code in [400, 401, 403, 404, 200] {
            assert!(!policy.should_retry(code), "expected {code} to NOT retry");
        }
        assert_eq!(policy.classify(200), None);
    }

    #[test]
    fn edge_client_retries_transient_edge_codes_but_not_origin_tls() {
        let policy = RetryPolicy::edge_client();
        for code in [429, 500, 520, 521, 522, 523, 524, 529, 530] {
            assert!(policy.should_retry(code), "expected {code} to retry");
        }
        for code in [525, 526] {
            assert_eq!(
                policy.classify(code),
                Some(Disposition::Terminal),
                "origin-TLS {code} must be terminal"
            );
        }
    }

    #[test]
    fn client_storage_classify() {
        let policy = RetryPolicy::client_storage();
        for code in [400, 403, 404, 525, 526] {
            assert_eq!(policy.classify(code), Some(Disposition::Terminal));
        }
        assert_eq!(policy.classify(401), Some(Disposition::AuthRefresh));
        for code in [429, 500, 503, 522, 409, 422] {
            assert_eq!(policy.classify(code), Some(Disposition::Retryable));
        }
        assert_eq!(policy.classify(200), None);
    }
}

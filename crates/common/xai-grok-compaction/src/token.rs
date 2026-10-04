//! Token-count seam.
//!

/// Counts tokens for a single conversation item on behalf of the shared
/// budgeting logic.
pub trait ItemTokenCounter<T: ?Sized>: Send + Sync {
    /// Trusted token count of `item`.
    fn count_item_tokens(&self, item: &T) -> u32;
}

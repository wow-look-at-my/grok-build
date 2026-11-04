// session.rs holds the session code.

use std::collections::{HashSet, VecDeque};
use std::sync::LazyLock;

use parking_lot::RwLock;

/// A shorter value would match ordinary header text by chance.
const MIN_TOKEN_LEN: usize = 8;

/// Most recent tokens kept. A refresh adds one; the oldest goes first.
const MAX_TOKENS: usize = 64;

#[derive(Default)]
struct Tokens {
    order: VecDeque<String>,
    set: HashSet<String>,
}

static TOKENS: LazyLock<RwLock<Tokens>> = LazyLock::new(Default::default);

/// Record an xAI session token this process holds. A request to a host xAI does not
/// operate is refused when any of its headers carries a recorded token.
/// The login crate calls this for every token it reads, mints or refreshes.
pub fn register_session_token(token: &str) {
    let token = token.trim();
    if token.len() < MIN_TOKEN_LEN {
        return;
    }
    let mut tokens = TOKENS.write();
    if !tokens.set.insert(token.to_owned()) {
        return;
    }
    tokens.order.push_back(token.to_owned());
    if tokens.order.len() > MAX_TOKENS
        && let Some(oldest) = tokens.order.pop_front()
    {
        tokens.set.remove(&oldest);
    }
}

pub(crate) fn carries_session_token(value: &[u8]) -> bool {
    TOKENS
        .read()
        .order
        .iter()
        .any(|token| value.windows(token.len()).any(|w| w == token.as_bytes()))
}

//! Which tool ids a session's MCP servers may advertise.

use std::collections::{HashMap, HashSet};

use xai_tool_protocol::ToolId;

/// Where a server's tools rank when ids collide. Ordered: a lower tier claims
/// first and wins collisions with a higher one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum McpServerTier {
    /// A local app endpoint named in the bind config's first-party set.
    FirstParty,
    /// Every other server, including every user-configured one.
    ThirdParty,
}

/// One server's offer: its configured name, tier, and the ids it exposes in
/// the server's own order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimOffer {
    pub(crate) name: String,
    pub(crate) tier: McpServerTier,
    pub(crate) tool_ids: Vec<ToolId>,
}

/// Ids each server may advertise, first-party first then name order, plus the
/// counts the caller logs.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ClaimPlan {
    pub(crate) claims: Vec<(String, Vec<ToolId>)>,
    /// Ids refused for a collision.
    pub(crate) rejected: usize,
    /// Ids refused only because the session's cap was already spent.
    pub(crate) over_cap: usize,
}

pub(crate) fn plan_claims(
    mut offers: Vec<ClaimOffer>,
    native: &HashSet<ToolId>,
    cap: usize,
) -> ClaimPlan {
    offers.sort_by(|left, right| {
        left.tier
            .cmp(&right.tier)
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut first_party_offers: HashMap<&ToolId, usize> = HashMap::new();
    let mut third_party_offers: HashMap<&ToolId, usize> = HashMap::new();
    for offer in &offers {
        let counts = match offer.tier {
            McpServerTier::FirstParty => &mut first_party_offers,
            McpServerTier::ThirdParty => &mut third_party_offers,
        };
        for tool_id in &offer.tool_ids {
            *counts.entry(tool_id).or_default() += 1;
        }
    }
    let is_claimable = |tier: McpServerTier, tool_id: &ToolId| {
        if native.contains(tool_id) {
            return false;
        }
        match tier {
            McpServerTier::FirstParty => first_party_offers.get(tool_id) == Some(&1),
            McpServerTier::ThirdParty => {
                !first_party_offers.contains_key(tool_id)
                    && third_party_offers.get(tool_id) == Some(&1)
            }
        }
    };

    let mut plan = ClaimPlan::default();
    let mut advertised = 0usize;
    for offer in &offers {
        let mut claimed = Vec::new();
        for tool_id in &offer.tool_ids {
            if !is_claimable(offer.tier, tool_id) {
                plan.rejected += 1;
                continue;
            }
            if advertised >= cap {
                plan.over_cap += 1;
                continue;
            }
            advertised += 1;
            claimed.push(tool_id.clone());
        }
        plan.claims.push((offer.name.clone(), claimed));
    }
    plan
}

#[cfg(test)]
#[path = "mcp_claim_tests.rs"]
mod tests;

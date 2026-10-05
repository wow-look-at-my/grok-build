use super::types::WebSearchConfig;
use crate::attribution::{SharedAttributionCallback, ToolConsumer};
use crate::types::SharedApiKeyProvider;
use async_openai::types::responses as rs;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
/// A minimal, purpose-built HTTP client for calling the Responses API
/// with web search capability.
/// Which API the configured provider speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchBackend {
    /// A model synthesizes an answer over the Responses API (`/responses`).
    Responses,
    /// Kagi's own ranked results (`/search`). No model is involved: Kagi
    /// returns them already filtered, ranked, and snippet-ed.
    Kagi,
}

/// Refuse a search request to an endpoint missing from `[endpoints] allowed_endpoints`.
fn allow_endpoint(url: &str) -> Result<(), xai_tool_runtime::ToolError> {
    xai_grok_extra_ca::endpoint_allowlist::check(url).map_err(|refusal| {
        xai_tool_runtime::ToolError::execution(
            xai_tool_protocol::ToolId::new("web_search").expect("valid"),
            refusal.to_string(),
        )
    })
}

#[derive(Clone)]
pub struct WebSearchClient {
    http: reqwest::Client,
    base_url: String,
    /// Synthesis model. Empty for [`SearchBackend::Kagi`], which has none.
    model: String,
    backend: SearchBackend,
    /// Results per query on [`SearchBackend::Kagi`]; `None` uses Kagi's default.
    kagi_limit: Option<usize>,
    /// Authoritative domain allowlist from `[toolset.web_search] allowed_domains`. When set it
    /// governs the search and the model's per-call `allowed_domains` is ignored (see
    /// [`Self::resolve_filters`]). Mutually exclusive with `default_excluded_domains`.
    default_allowed_domains: Option<Vec<String>>,
    /// Authoritative domain blocklist from `[toolset.web_search] excluded_domains`.
    /// The model cannot un-set it by naming a blocked domain in its own
    /// `allowed_domains`. Mutually exclusive with `default_allowed_domains`.
    default_excluded_domains: Option<Vec<String>>,
    api_key_provider: Option<SharedApiKeyProvider>,
    /// Optional 401-attribution hook. Callers can wire this so a 401
    /// from the Responses API emits an `auth_401_attribution` event
    /// with `consumer == "WebSearch"`.
    attribution_callback: Option<SharedAttributionCallback>,
}
impl WebSearchClient {
    /// Create a new web search client from `WebSearchConfig::Enabled`.
    ///
    /// Returns `Err` if the config is `Disabled` or if header values are invalid.
    pub fn new(
        config: &WebSearchConfig,
        api_key_provider: Option<SharedApiKeyProvider>,
    ) -> Result<Self, xai_tool_runtime::ToolError> {
        // Each arm carries its own auth scheme: the Responses API wants a
        // bearer token, Kagi's Search API wants `Bot <token>`.
        let (
            api_key,
            base_url,
            model,
            extra_headers,
            backend,
            kagi_limit,
            scheme,
            allowed_domains,
            excluded_domains,
        ) = match config {
            WebSearchConfig::Disabled => {
                return Err(xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    "Cannot create WebSearchClient from disabled config".to_string(),
                ));
            }
            WebSearchConfig::Enabled {
                api_key,
                base_url,
                model,
                extra_headers,
                alpha_test_key,
                allowed_domains,
                excluded_domains,
            } => {
                let _ = alpha_test_key;
                (
                    api_key,
                    base_url,
                    model.clone(),
                    extra_headers,
                    SearchBackend::Responses,
                    None,
                    "Bearer",
                    allowed_domains.clone(),
                    excluded_domains.clone(),
                )
            }
            WebSearchConfig::Kagi {
                api_key,
                base_url,
                limit,
                extra_headers,
            } => (
                api_key,
                base_url,
                String::new(),
                extra_headers,
                SearchBackend::Kagi,
                *limit,
                "Bot",
                None,
                None,
            ),
        };
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("{scheme} {api_key}")).map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("Invalid API key for header: {e}"),
                )
            })?,
        );
        for (key, value) in extra_headers {
            let header_name = HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("Invalid header name '{key}': {e}"),
                )
            })?;
            let header_value = HeaderValue::from_str(value).map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("Invalid header value for '{key}': {e}"),
                )
            })?;
            headers.insert(header_name, header_value);
        }
        let key = crate::util::shared_http::cache_key("web_search", &headers);
        let http = crate::util::shared_http::cached_client(key, || {
            xai_grok_extra_ca::build_reqwest_client(|builder| {
                builder.default_headers(headers.clone())
            })
        })
        .map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to build HTTP client: {e}"),
            )
        })?;
        Ok(Self {
            http,
            base_url: base_url.clone(),
            model,
            backend,
            kagi_limit,
            default_allowed_domains: allowed_domains,
            default_excluded_domains: excluded_domains,
            api_key_provider,
            attribution_callback: None,
        })
    }
    /// Resolve the effective domain filters for a request. This is required for `excluded_domains` to be a real block. Otherwise the model could
    /// bypass the user's blocklist simply by naming the blocked domain in its own `allowed_domains`. Only when no config policy is set does the
    /// model's per-call allowlist apply. The two lists are mutually exclusive, so at most one of the returned options is `Some`.
    fn resolve_filters(
        &self,
        model_allowed: Option<Vec<String>>,
    ) -> (Option<Vec<String>>, Option<Vec<String>>) {
        if let Some(allowed) = self
            .default_allowed_domains
            .clone()
            .filter(|d| !d.is_empty())
        {
            return (Some(allowed), None);
        }
        if let Some(excluded) = self
            .default_excluded_domains
            .clone()
            .filter(|d| !d.is_empty())
        {
            return (None, Some(excluded));
        }
        (model_allowed.filter(|d| !d.is_empty()), None)
    }
    /// Build the serialized `/responses` request body for a single web search. The request always
    /// carries exactly one tool (`web_search`) at index 0.
    fn build_request_json(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
        excluded_domains: Option<Vec<String>>,
    ) -> Result<serde_json::Value, xai_tool_runtime::ToolError> {
        let err = |msg: String| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                msg,
            )
        };
        let web_search = rs::WebSearchToolArgs::default()
            .filters(rs::WebSearchToolFilters { allowed_domains })
            .build()
            .map_err(|e| err(format!("Failed to build web search tool: {e}")))?;
        let request = rs::CreateResponseArgs::default()
            .model(self.model.clone())
            .input(query.to_string())
            .tools(vec![rs::Tool::WebSearch(web_search)])
            .store(false)
            .temperature(0.1)
            .top_p(0.95)
            .max_output_tokens(8192u32)
            .build()
            .map_err(|e| err(format!("Failed to build request: {e}")))?;
        let mut body = serde_json::to_value(&request)
            .map_err(|e| err(format!("Failed to serialize request: {e}")))?;
        if let Some(excluded) = excluded_domains.filter(|d| !d.is_empty()) {
            let tool = body
                .get_mut("tools")
                .and_then(|t| t.as_array_mut())
                .and_then(|arr| arr.first_mut())
                .and_then(|t| t.as_object_mut());
            if let Some(tool) = tool {
                let filters = tool
                    .entry("filters")
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(obj) = filters.as_object_mut() {
                    obj.insert("excluded_domains".to_owned(), serde_json::json!(excluded));
                }
            }
        }
        Ok(body)
    }
    /// Wire a 401-attribution callback into this client. Idempotent;
    /// safe to call before or after the first request.
    pub fn with_attribution_callback(
        mut self,
        callback: Option<SharedAttributionCallback>,
    ) -> Self {
        self.attribution_callback = callback;
        self
    }
    async fn current_bearer(&self) -> Option<String> {
        crate::types::api_key_provider::resolve_bearer(self.api_key_provider.as_ref()).await
    }
    fn record_401_attribution(&self, sent_bearer: Option<&str>) {
        crate::attribution::emit_401(
            self.attribution_callback.as_ref(),
            ToolConsumer::WebSearch,
            sent_bearer,
        );
    }
    /// Perform a web search query using the Responses API. Returns `(content, citations)` where
    /// content is the assistant's text and citations are unique URLs found in the response
    /// annotations.
    pub async fn search(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>), xai_tool_runtime::ToolError> {
        if self.backend == SearchBackend::Kagi {
            let (content, pairs) = self.kagi_results(query, allowed_domains).await?;
            return Ok((
                content,
                pairs.into_iter().map(|(_title, url)| url).collect(),
            ));
        }
        let (allowed, excluded) = self.resolve_filters(allowed_domains);
        let request = self.build_request_json(query, allowed, excluded)?;
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        allow_endpoint(&url)?;
        let sent_bearer = self.current_bearer().await;
        let mut req = self.http.post(&url).json(&request);
        if let Some(ref key) = sent_bearer {
            req = req.header(AUTHORIZATION, format!("Bearer {key}"));
        }
        let response = req.send().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("HTTP request failed: {e}"),
            )
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.record_401_attribution(sent_bearer.as_deref());
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::unauthorized(format!(
                "Responses API returned 401 Unauthorized: {body}"
            ))
            .with_details(serde_json::json!({
                "tool_id": "web_search",
                "status": 401,
            })));
        }
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Responses API returned {status}: {body}"),
            ));
        }
        let bytes = response.bytes().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to read response body: {e}"),
            )
        })?;
        let response_obj: rs::Response = serde_json::from_slice(&bytes).map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to parse response: {e}"),
            )
        })?;
        let content = response_obj
            .output_text()
            .unwrap_or_else(|| "No search results found.".to_string());
        let citations = extract_citations(&response_obj);
        Ok((content, citations))
    }
    /// Same as [`Self::search`] but also extracts per-citation titles when the Responses API surfaces them. Returns `(content,
    /// citations_with_titles)` where each citation is `(title, url)`. Empty `title` strings indicate the upstream didn't supply one for that URL.
    /// Used by the cursor-compat `WebSearch` adapter to render a `Links:\n1. [title](url)` list instead of the LLM synthesis text.
    pub async fn search_with_titles(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<(String, String)>), xai_tool_runtime::ToolError> {
        if self.backend == SearchBackend::Kagi {
            return self.kagi_results(query, allowed_domains).await;
        }
        let (allowed, excluded) = self.resolve_filters(allowed_domains);
        let request = self.build_request_json(query, allowed, excluded)?;
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        allow_endpoint(&url)?;
        let sent_bearer = self.current_bearer().await;
        let mut req = self.http.post(&url).json(&request);
        if let Some(ref key) = sent_bearer {
            req = req.header(AUTHORIZATION, format!("Bearer {key}"));
        }
        let response = req.send().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("HTTP request failed: {e}"),
            )
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.record_401_attribution(sent_bearer.as_deref());
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::unauthorized(format!(
                "Responses API returned 401 Unauthorized: {body}"
            ))
            .with_details(serde_json::json!({
                "tool_id": "web_search",
                "status": 401,
            })));
        }
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Responses API returned {status}: {body}"),
            ));
        }
        let bytes = response.bytes().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to read response body: {e}"),
            )
        })?;
        let response_obj: rs::Response = serde_json::from_slice(&bytes).map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to parse response: {e}"),
            )
        })?;
        let content = response_obj
            .output_text()
            .unwrap_or_else(|| "No search results found.".to_string());
        let pairs = extract_citation_pairs(&response_obj);
        Ok((content, pairs))
    }

    // ── Kagi backend ────────────────────────────────────────────────────

    /// Fetch Kagi's ranked results and render them as `(content, (title, url))`.
    ///
    /// Nothing is synthesized: Kagi returns these already ranked, filtered, and
    /// snippet-ed, so the payload is the results themselves.
    async fn kagi_results(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<(String, String)>), xai_tool_runtime::ToolError> {
        let body = self.fetch_kagi(query).await?;
        let allowed = allowed_domains.as_deref();
        let results: Vec<&KagiItem> = body
            .data
            .iter()
            .filter(|item| item.t == KAGI_RESULT)
            .filter(|item| {
                item.url
                    .as_deref()
                    .is_some_and(|url| kagi_domain_allowed(url, allowed))
            })
            .collect();
        let related: Vec<&str> = body
            .data
            .iter()
            .filter(|item| item.t == KAGI_RELATED)
            .flat_map(|item| item.list.iter().map(String::as_str))
            .collect();
        Ok(format_kagi_results(&results, &related))
    }

    /// One `GET /search` against Kagi's Search API.
    async fn fetch_kagi(&self, query: &str) -> Result<KagiSearchBody, xai_tool_runtime::ToolError> {
        let url = format!("{}/search", self.base_url.trim_end_matches('/'));
        allow_endpoint(&url)?;
        let mut params: Vec<(&str, String)> = vec![("q", query.to_string())];
        if let Some(limit) = self.kagi_limit {
            params.push(("limit", limit.to_string()));
        }
        let response = self
            .http
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| kagi_error(format!("Kagi search request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(kagi_error(format!("Kagi search returned {status}: {body}")));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| kagi_error(format!("Failed to read Kagi response body: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| kagi_error(format!("Failed to parse Kagi response: {e}")))
    }
}

/// A `web_search` tool error tagged with the tool id.
fn kagi_error(message: String) -> xai_tool_runtime::ToolError {
    xai_tool_runtime::ToolError::execution(
        xai_tool_protocol::ToolId::new("web_search").expect("valid"),
        message,
    )
}

/// Kagi `data` entry type: a search result.
const KAGI_RESULT: i64 = 0;
/// Kagi `data` entry type: a related-searches list.
const KAGI_RELATED: i64 = 1;

/// One Kagi Search API response body.
///
/// Kagi types each `data` entry with an integer `t`, so the fields are modelled
/// flat and matched on `t` rather than as a serde-tagged enum (serde's internal
/// tagging wants a string tag).
#[derive(Debug, serde::Deserialize)]
struct KagiSearchBody {
    #[serde(default)]
    data: Vec<KagiItem>,
}

#[derive(Debug, serde::Deserialize)]
struct KagiItem {
    t: i64,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    list: Vec<String>,
}

/// Render Kagi's results as the text the model reads, plus the citation pairs.
///
/// Each result becomes its title, URL, and snippet, so the model sees what Kagi
/// returned without a synthesis pass. Related searches carry no URL, so they
/// ride the text and never a citation.
fn format_kagi_results(results: &[&KagiItem], related: &[&str]) -> (String, Vec<(String, String)>) {
    let mut blocks = Vec::with_capacity(results.len() + 1);
    let mut pairs = Vec::with_capacity(results.len());
    for item in results {
        let Some(url) = item.url.as_deref() else {
            continue;
        };
        let title = item.title.as_deref().unwrap_or(url);
        pairs.push((title.to_string(), url.to_string()));
        match item
            .snippet
            .as_deref()
            .filter(|snippet| !snippet.is_empty())
        {
            Some(snippet) => blocks.push(format!("{title}\n{url}\n{snippet}")),
            None => blocks.push(format!("{title}\n{url}")),
        }
    }
    if !related.is_empty() {
        blocks.push(format!("Related searches: {}", related.join(", ")));
    }
    if blocks.is_empty() {
        return ("No search results found.".to_string(), Vec::new());
    }
    (blocks.join("\n\n"), pairs)
}

/// Whether `url`'s host falls under any of the caller's `allowed_domains`.
///
/// Kagi's Search API takes no per-request domain filter, so the tool's
/// `allowed_domains` argument is applied here rather than silently dropped.
/// `None` (or an empty list) is unrestricted; a subdomain of an allowed domain
/// matches, as the Responses-API filter does.
fn kagi_domain_allowed(url: &str, allowed: Option<&[String]>) -> bool {
    let Some(allowed) = allowed.filter(|list| !list.is_empty()) else {
        return true;
    };
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches("www.");
    allowed.iter().any(|domain| {
        let domain = domain.trim().trim_start_matches("www.");
        !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
    })
}
/// Extract citation URLs from the Response output items.
/// The async-openai crate doesn't provide a helper for this, and the `url` field
/// in `UrlCitationBody` is private, so we serialize to JSON to extract it.
fn extract_citations(response: &rs::Response) -> Vec<String> {
    let mut citations = Vec::new();
    for output_item in &response.output {
        if let rs::OutputItem::Message(output_message) = output_item {
            for message_content in &output_message.content {
                if let rs::OutputMessageContent::OutputText(text_content) = message_content {
                    for annotation in &text_content.annotations {
                        if let rs::Annotation::UrlCitation(url_citation) = annotation
                            && let Ok(json) = serde_json::to_value(url_citation)
                            && let Some(url) = json.get("url").and_then(|v| v.as_str())
                        {
                            citations.push(url.to_string());
                        }
                    }
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    citations.retain(|url| seen.insert(url.clone()));
    citations
}
/// Extract `(title, url)` pairs from the Responses API annotations. `title` may be an empty string
/// when upstream doesn't supply one. URLs are deduplicated while preserving the first-seen order so
/// the rendered `Links:` list is stable and free of duplicates.
fn extract_citation_pairs(response: &rs::Response) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for output_item in &response.output {
        if let rs::OutputItem::Message(output_message) = output_item {
            for message_content in &output_message.content {
                if let rs::OutputMessageContent::OutputText(text_content) = message_content {
                    for annotation in &text_content.annotations {
                        if let rs::Annotation::UrlCitation(url_citation) = annotation
                            && let Ok(json) = serde_json::to_value(url_citation)
                        {
                            let url = json.get("url").and_then(|v| v.as_str()).unwrap_or("");
                            if url.is_empty() {
                                continue;
                            }
                            let title = json
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            pairs.push((title, url.to_string()));
                        }
                    }
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    pairs.retain(|(_t, url)| seen.insert(url.clone()));
    pairs
}
#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    /// Helper to create a Response from JSON for testing.
    fn response_from_json(json: serde_json::Value) -> rs::Response {
        serde_json::from_value(json).expect("Failed to parse test Response JSON")
    }
    /// Build a client with the given configured domain defaults.
    fn client_with_defaults(
        allowed: Option<Vec<String>>,
        excluded: Option<Vec<String>>,
    ) -> WebSearchClient {
        let config = WebSearchConfig::Enabled {
            api_key: "test-key".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: allowed,
            excluded_domains: excluded,
        };
        WebSearchClient::new(&config, None).expect("client should build")
    }
    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn resolve_filters_config_allowlist_wins_over_model() {
        let client = client_with_defaults(Some(v(&["config.com"])), None);
        let (allowed, excluded) = client.resolve_filters(Some(v(&["model.com"])));
        assert_eq!(allowed, Some(v(&["config.com"])));
        assert!(excluded.is_none());
    }
    #[test]
    fn resolve_filters_uses_config_allowlist_when_model_silent() {
        let client = client_with_defaults(Some(v(&["config.com"])), None);
        let (allowed, excluded) = client.resolve_filters(None);
        assert_eq!(allowed, Some(v(&["config.com"])));
        assert!(excluded.is_none());
    }
    #[test]
    fn resolve_filters_config_blocklist_applies_when_no_allowlist() {
        let client = client_with_defaults(None, Some(v(&["reddit.com"])));
        let (allowed, excluded) = client.resolve_filters(None);
        assert!(allowed.is_none());
        assert_eq!(excluded, Some(v(&["reddit.com"])));
    }
    #[test]
    fn resolve_filters_config_blocklist_cannot_be_bypassed_by_model() {
        let client = client_with_defaults(None, Some(v(&["github.com"])));
        let (allowed, excluded) = client.resolve_filters(Some(v(&["github.com"])));
        assert!(
            allowed.is_none(),
            "model allowlist must not override the block"
        );
        assert_eq!(excluded, Some(v(&["github.com"])));
    }
    #[test]
    fn resolve_filters_no_config_honors_model_allowlist() {
        let client = client_with_defaults(None, None);
        let (allowed, excluded) = client.resolve_filters(Some(v(&["model.com"])));
        assert_eq!(allowed, Some(v(&["model.com"])));
        assert!(excluded.is_none());
    }
    #[test]
    fn build_request_json_injects_excluded_domains() {
        let client = client_with_defaults(None, None);
        let body = client
            .build_request_json("q", None, Some(v(&["reddit.com"])))
            .expect("request json builds");
        let Some(filters) = body.pointer("/tools/0/filters") else {
            panic!("missing tools[0].filters: {body}");
        };
        assert_eq!(
            filters.get("excluded_domains"),
            Some(&serde_json::json!(["reddit.com"]))
        );
        assert!(filters.get("allowed_domains").is_none());
    }
    #[test]
    fn build_request_json_allowlist_only_has_no_excluded_key() {
        let client = client_with_defaults(None, None);
        let body = client
            .build_request_json("q", Some(v(&["docs.x.ai"])), None)
            .expect("request json builds");
        let Some(filters) = body.pointer("/tools/0/filters") else {
            panic!("missing tools[0].filters: {body}");
        };
        assert_eq!(
            filters.get("allowed_domains"),
            Some(&serde_json::json!(["docs.x.ai"]))
        );
        assert!(filters.get("excluded_domains").is_none());
    }
    #[test]
    fn test_new_client_uses_configured_model() {
        let config = WebSearchConfig::Enabled {
            api_key: "test-key".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "custom-enterprise-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        assert_eq!(client.model, "custom-enterprise-model");
    }
    /// Counts attribution callback invocations for the test below.
    #[derive(Default, Debug)]
    struct CountingCallback {
        invocations: std::sync::Mutex<Vec<(ToolConsumer, Option<String>)>>,
    }
    impl crate::attribution::Auth401AttributionCallback for CountingCallback {
        fn record_401(&self, consumer: ToolConsumer, sent_bearer_suffix: Option<&str>) {
            self.invocations
                .lock()
                .unwrap()
                .push((consumer, sent_bearer_suffix.map(|s| s.to_string())));
        }
    }
    /// `record_401_attribution` invokes the wired callback with
    /// `ToolConsumer::WebSearch` and the truncated bearer prefix.
    /// The full bearer never crosses the trait boundary.
    #[test]
    fn record_401_attribution_passes_truncated_prefix_to_callback() {
        let cb = std::sync::Arc::new(CountingCallback::default());
        let cb_dyn: crate::attribution::SharedAttributionCallback = cb.clone();
        let config = WebSearchConfig::Enabled {
            api_key: "ignored".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
        };
        let client = WebSearchClient::new(&config, None)
            .expect("client should build")
            .with_attribution_callback(Some(cb_dyn));
        client.record_401_attribution(Some("bearer-with-long-tail-aaaadistinct"));
        let calls = cb.invocations.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let Some(call) = calls.first() else {
            panic!("expected one attribution call");
        };
        assert_eq!(call.0, ToolConsumer::WebSearch);
        assert_eq!(call.1.as_deref(), Some("aaaadistinct"));
        assert_eq!(
            call.1.as_deref().map(str::len),
            Some(crate::attribution::BEARER_SUFFIX_LEN),
        );
    }
    /// `record_401_attribution` is a no-op when no callback is wired
    /// -- the BYOK / standalone case must not panic or allocate.
    #[test]
    fn record_401_attribution_is_noop_without_callback() {
        let config = WebSearchConfig::Enabled {
            api_key: "test-key".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        client.record_401_attribution(Some("any-bearer"));
        client.record_401_attribution(None);
    }
    #[test]
    fn test_extract_citations_empty_response() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "output": [],
            "model": "test-model"
        }));
        let citations = extract_citations(&response);
        assert!(citations.is_empty());
    }
    #[test]
    fn test_extract_citations_with_url_citations() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Here is some info about Rust.",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://www.rust-lang.org/",
                                    "title": "Rust Programming Language",
                                    "start_index": 0,
                                    "end_index": 10
                                },
                                {
                                    "type": "url_citation",
                                    "url": "https://docs.rs/",
                                    "title": "Docs.rs",
                                    "start_index": 11,
                                    "end_index": 20
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert_eq!(citations.len(), 2);
        let [first, second] = citations.as_slice() else {
            panic!("expected 2 citations: {citations:?}");
        };
        assert_eq!(first, "https://www.rust-lang.org/");
        assert_eq!(second, "https://docs.rs/");
    }
    #[test]
    fn test_extract_citations_deduplicates() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Info with duplicate citations.",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://example.com/page1",
                                    "title": "Page 1",
                                    "start_index": 0,
                                    "end_index": 5
                                },
                                {
                                    "type": "url_citation",
                                    "url": "https://example.com/page2",
                                    "title": "Page 2",
                                    "start_index": 6,
                                    "end_index": 10
                                },
                                {
                                    "type": "url_citation",
                                    "url": "https://example.com/page1",
                                    "title": "Page 1 Again",
                                    "start_index": 11,
                                    "end_index": 15
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert_eq!(citations.len(), 2);
        let [first, second] = citations.as_slice() else {
            panic!("expected 2 citations: {citations:?}");
        };
        assert_eq!(first, "https://example.com/page1");
        assert_eq!(second, "https://example.com/page2");
    }
    #[test]
    fn test_extract_citations_multiple_messages() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "First message",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://first.com/",
                                    "title": "First",
                                    "start_index": 0,
                                    "end_index": 5
                                }
                            ]
                        }
                    ]
                },
                {
                    "type": "message",
                    "id": "msg_2",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Second message",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://second.com/",
                                    "title": "Second",
                                    "start_index": 0,
                                    "end_index": 6
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert_eq!(citations.len(), 2);
        let [first, second] = citations.as_slice() else {
            panic!("expected 2 citations: {citations:?}");
        };
        assert_eq!(first, "https://first.com/");
        assert_eq!(second, "https://second.com/");
    }
    #[test]
    fn test_extract_citations_ignores_non_url_annotations() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Some text",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://valid.com/",
                                    "title": "Valid",
                                    "start_index": 0,
                                    "end_index": 4
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert_eq!(citations.len(), 1);
        assert_eq!(
            citations.first().map(String::as_str),
            Some("https://valid.com/")
        );
    }
    /// A provider that always returns `None`, simulating an API-key user
    /// whose token has aged past the client-side TTL.
    struct NoneProvider;
    impl crate::types::ApiKeyProvider for NoneProvider {
        fn current_api_key(&self) -> Option<String> {
            None
        }
    }
    /// When the dynamic provider returns `None`, the static `api_key` from config must still be
    /// sent as the Authorization header. This is a regression scenario: API-key users past the
    /// 30-day client TTL saw 401 because no auth was sent.
    #[tokio::test]
    async fn static_api_key_is_fallback_when_provider_returns_none() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("Authorization", "Bearer static-key-from-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_test",
                "object": "response",
                "created_at": 1234567890,
                "status": "completed",
                "model": "test-model",
                "output": [{
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "search result",
                        "annotations": []
                    }]
                }]
            })))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Enabled {
            api_key: "static-key-from-config".to_string(),
            base_url: server.uri(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
        };
        let provider: SharedApiKeyProvider = std::sync::Arc::new(NoneProvider);
        let client = WebSearchClient::new(&config, Some(provider)).expect("client should build");
        let (content, _citations) = client
            .search("test query", None)
            .await
            .expect("search must succeed with static key fallback");
        assert_eq!(content, "search result");
    }
    /// When the provider returns a fresh key, it overrides the static one.
    #[tokio::test]
    async fn provider_key_overrides_static_key() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        struct FreshProvider;
        impl crate::types::ApiKeyProvider for FreshProvider {
            fn current_api_key(&self) -> Option<String> {
                Some("fresh-key-from-provider".to_string())
            }
        }
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("Authorization", "Bearer fresh-key-from-provider"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_test",
                "object": "response",
                "created_at": 1234567890,
                "status": "completed",
                "model": "test-model",
                "output": [{
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "fresh result",
                        "annotations": []
                    }]
                }]
            })))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Enabled {
            api_key: "stale-static-key".to_string(),
            base_url: server.uri(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
        };
        let provider: SharedApiKeyProvider = std::sync::Arc::new(FreshProvider);
        let client = WebSearchClient::new(&config, Some(provider)).expect("client should build");
        let (content, _citations) = client
            .search("test query", None)
            .await
            .expect("search must succeed with provider key");
        assert_eq!(content, "fresh result");
    }
    #[test]
    fn test_extract_citations_no_annotations() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Plain text with no annotations",
                            "annotations": []
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert!(citations.is_empty());
    }

    // ── Kagi backend ────────────────────────────────────────────────────

    /// The Kagi backend is one `GET /search` carrying the `Bot` scheme, and the
    /// text the tool returns is Kagi's own results — no model call is made.
    #[tokio::test]
    async fn kagi_backend_requests_search_and_renders_results() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "borrow checker"))
            .and(query_param("limit", "3"))
            .and(header("Authorization", "Bot kagi-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "meta": { "id": "m", "node": "us", "ms": 12 },
                "data": [
                    {
                        "t": 0,
                        "url": "https://doc.rust-lang.org/nomicon/borrow-splitting.html",
                        "title": "Borrow splitting",
                        "snippet": "Splitting borrows is sound."
                    },
                    { "t": 1, "list": ["nll", "polonius"] }
                ]
            })))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Kagi {
            api_key: "kagi-token".to_string(),
            base_url: server.uri(),
            limit: Some(3),
            extra_headers: IndexMap::new(),
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let (content, citations) = client
            .search("borrow checker", None)
            .await
            .expect("kagi search must succeed");
        assert!(content.contains("Borrow splitting"), "content: {content}");
        assert!(
            content.contains("Splitting borrows is sound."),
            "content: {content}"
        );
        assert!(
            content.contains("Related searches: nll, polonius"),
            "content: {content}"
        );
        assert_eq!(
            citations,
            vec!["https://doc.rust-lang.org/nomicon/borrow-splitting.html".to_string()]
        );
    }

    /// Kagi's Search API takes no per-request domain filter, so the tool's
    /// `allowed_domains` argument is applied to the returned results rather
    /// than silently dropped.
    #[tokio::test]
    async fn kagi_backend_applies_allowed_domains() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    { "t": 0, "url": "https://docs.rs/serde/latest/serde/", "title": "serde" },
                    { "t": 0, "url": "https://example.com/other", "title": "other" }
                ]
            })))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Kagi {
            api_key: "k".to_string(),
            base_url: server.uri(),
            limit: None,
            extra_headers: IndexMap::new(),
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let (content, citations) = client
            .search("serde", Some(vec!["docs.rs".to_string()]))
            .await
            .expect("kagi search must succeed");
        assert_eq!(
            citations,
            vec!["https://docs.rs/serde/latest/serde/".to_string()]
        );
        assert!(!content.contains("example.com"), "content: {content}");
    }

    /// `search_with_titles` keeps the titles Kagi supplied.
    #[tokio::test]
    async fn kagi_backend_returns_title_url_pairs() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "t": 0, "url": "https://a.example/x", "title": "A page" }]
            })))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Kagi {
            api_key: "k".to_string(),
            base_url: server.uri(),
            limit: None,
            extra_headers: IndexMap::new(),
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let (content, pairs) = client
            .search_with_titles("anything", None)
            .await
            .expect("kagi search must succeed");
        assert_eq!(
            pairs,
            vec![("A page".to_string(), "https://a.example/x".to_string())]
        );
        assert!(content.contains("A page"));
    }

    /// A Kagi failure surfaces as a Kagi error, never as a silent empty result.
    #[tokio::test]
    async fn kagi_backend_surfaces_http_errors() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid token"))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Kagi {
            api_key: "bad".to_string(),
            base_url: server.uri(),
            limit: None,
            extra_headers: IndexMap::new(),
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let err = client
            .search("q", None)
            .await
            .expect_err("a 401 must be an error");
        let text = format!("{err:?}");
        assert!(text.contains("Kagi search returned"), "err: {text}");
        assert!(text.contains("invalid token"), "err: {text}");
    }
}

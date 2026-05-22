//! Pure utility helpers for the proxy dispatcher.
//!
//! Covers: route-target loading, retryability, cache configuration helpers,
//! request introspection, and semantic embedding input extraction.

use tokio::time::Duration;

use reqwest::header::{
    HeaderMap as ReqwestHeaderMap, HeaderName as ReqwestHeaderName,
    HeaderValue as ReqwestHeaderValue,
};

use crate::Gateway;
use crate::cache::entry::CacheEntry;
use crate::db::models::{
    Route, RouteCacheConfig, RouteExactCacheConfig, RouteSemanticCacheConfig, RouteTarget,
};
use crate::protocol::ir::{AiRequest, ContentBlock, MessageContent, Role};

// ── Semantic write context ─────────────────────────────────────────────────────

/// Carry-along context that allows the response handler to write a semantic
/// cache entry after a successful upstream call.
#[derive(Clone)]
pub(super) struct SemanticWriteContext {
    pub(super) partition: String,
    pub(super) embedding_text: String,
    pub(super) key: String,
    pub(super) query_vector: Option<Vec<f32>>,
}

// ── Route target loading ────────────────────────────────────────────────────────

pub(super) async fn load_route_targets(gw: &Gateway, route: &Route) -> Vec<RouteTarget> {
    if let Some(store) = gw.storage.route_targets()
        && let Ok(targets) = store.list_targets_by_route(&route.id).await
        && !targets.is_empty()
    {
        return targets;
    }
    // Fallback: synthesize a single target from the legacy
    // `route.target_provider` / `route.target_model` columns.
    if route.target_provider.trim().is_empty() {
        return Vec::new();
    }
    vec![RouteTarget {
        id: String::new(),
        route_id: route.id.clone(),
        provider_id: route.target_provider.clone(),
        model: route.target_model.clone(),
        weight: 100,
        priority: 1,
        created_at: String::new(),
    }]
}

// ── Retry ─────────────────────────────────────────────────────────────────────

pub(super) fn is_retryable(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 529)
}

// ── Runtime-binding extra headers ─────────────────────────────────────────────

pub(super) fn runtime_binding_headers(
    binding: &crate::auth::RuntimeBinding,
) -> anyhow::Result<ReqwestHeaderMap> {
    let mut headers = ReqwestHeaderMap::new();
    for (key, value) in &binding.extra_headers {
        headers.insert(
            reqwest::header::HeaderName::from_bytes(key.as_bytes())?,
            ReqwestHeaderValue::from_str(value)?,
        );
    }
    Ok(headers)
}

/// Convert client-supplied request headers into the safe subset that may be
/// forwarded upstream.
///
/// Authentication, API-key, cookie, hop-by-hop, proxy, and client network
/// identity headers are intentionally dropped so Nyro's local credentials and
/// caller IP/host metadata never leak to providers. Provider/runtime headers
/// are merged elsewhere after this function, so internal credentials still win.
pub(super) fn forwarded_client_headers(headers: &axum::http::HeaderMap) -> ReqwestHeaderMap {
    let mut forwarded = ReqwestHeaderMap::new();
    for (name, value) in headers {
        if !should_forward_client_header(name.as_str()) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            ReqwestHeaderName::from_bytes(name.as_str().as_bytes()),
            ReqwestHeaderValue::from_bytes(value.as_bytes()),
        ) {
            forwarded.append(name, value);
        }
    }
    forwarded
}

fn should_forward_client_header(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return false;
    }

    let denied = matches!(
        name.as_str(),
        // Local/proxy credentials and cookies.
        "authorization"
            | "proxy-authorization"
            | "www-authenticate"
            | "proxy-authenticate"
            | "x-api-key"
            | "x-goog-api-key"
            | "api-key"
            | "x-auth-token"
            | "x-access-token"
            | "x-refresh-token"
            | "access-token"
            | "refresh-token"
            | "cookie"
            | "set-cookie"
            // Hop-by-hop / transport-managed headers.
            | "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
            | "accept-encoding"
            | "content-encoding"
            // Client network identity / local origin metadata.
            | "forwarded"
            | "x-forwarded-for"
            | "x-forwarded-host"
            | "x-forwarded-proto"
            | "x-forwarded-port"
            | "x-forwarded-server"
            | "x-original-forwarded-for"
            | "x-real-ip"
            | "x-client-ip"
            | "x-cluster-client-ip"
            | "x-remote-ip"
            | "x-remote-addr"
            | "remote-host"
            | "remote-addr"
            | "cf-connecting-ip"
            | "true-client-ip"
            | "fastly-client-ip"
            | "via"
            | "origin"
            | "referer"
    ) || name.ends_with("-api-key")
        || name.starts_with("sec-")
        || name.starts_with("proxy-")
        || name.starts_with("cf-");

    !denied
}

// ── Route-level cache configuration helpers ────────────────────────────────────

pub(super) fn resolve_route_cache(route: &Route) -> RouteCacheConfig {
    let exact = route.cache_exact_ttl.map(|ttl| RouteExactCacheConfig {
        ttl: if ttl > 0 { Some(ttl) } else { None },
    });
    let semantic = route
        .cache_semantic_ttl
        .map(|ttl| RouteSemanticCacheConfig {
            ttl: if ttl > 0 { Some(ttl) } else { None },
            threshold: route.cache_semantic_threshold,
        });
    RouteCacheConfig { exact, semantic }
}

pub(super) fn route_exact_ttl(cache: &RouteCacheConfig, default_ttl: Duration) -> Duration {
    cache
        .exact
        .as_ref()
        .and_then(|e| e.ttl)
        .and_then(|ttl| (ttl > 0).then_some(Duration::from_secs(ttl as u64)))
        .unwrap_or(default_ttl)
}

pub(super) fn route_semantic_ttl(cache: &RouteCacheConfig, default_ttl: Duration) -> Duration {
    cache
        .semantic
        .as_ref()
        .and_then(|s| s.ttl)
        .and_then(|ttl| (ttl > 0).then_some(Duration::from_secs(ttl as u64)))
        .unwrap_or(default_ttl)
}

pub(super) fn route_semantic_threshold(cache: &RouteCacheConfig, default_threshold: f64) -> f64 {
    cache
        .semantic
        .as_ref()
        .and_then(|s| s.threshold)
        .filter(|t| *t > 0.0)
        .unwrap_or(default_threshold)
}

pub(super) fn is_semantic_entry_expired(entry: &CacheEntry, ttl: Duration) -> bool {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let ttl_ms = ttl.as_millis() as i64;
    now_ms.saturating_sub(entry.created_at_epoch_ms) > ttl_ms
}

// ── Request introspection ─────────────────────────────────────────────────────

pub(super) fn request_has_image_input(request: &AiRequest) -> bool {
    for message in &request.messages {
        if let MessageContent::Blocks(blocks) = &message.content
            && blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. }))
        {
            return true;
        }
    }
    false
}

pub(super) fn extract_semantic_embedding_input(request: &AiRequest) -> Option<(String, String)> {
    let system_prompt = request
        .messages
        .iter()
        .filter(|m| matches!(m.role, Role::System))
        .map(|m| m.content.to_text())
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    let last_user = request
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.content.to_text())
        .filter(|t| !t.trim().is_empty())?;

    let combined = if system_prompt.is_empty() {
        last_user.clone()
    } else {
        format!("{system_prompt}\n{last_user}")
    };
    Some((system_prompt, combined))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::*;

    #[test]
    fn forwarded_client_headers_keep_cache_hints() {
        let mut headers = HeaderMap::new();
        headers.insert("anthropic-beta", HeaderValue::from_static("prompt-caching"));
        headers.insert("openai-beta", HeaderValue::from_static("responses=v1"));
        headers.insert("idempotency-key", HeaderValue::from_static("request-123"));

        let forwarded = forwarded_client_headers(&headers);

        assert_eq!(forwarded.get("anthropic-beta").unwrap(), "prompt-caching");
        assert_eq!(forwarded.get("openai-beta").unwrap(), "responses=v1");
        assert_eq!(forwarded.get("idempotency-key").unwrap(), "request-123");
    }

    #[test]
    fn forwarded_client_headers_drop_keys_and_sensitive_network_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer nyro-key"));
        headers.insert("x-api-key", HeaderValue::from_static("nyro-key"));
        headers.insert("x-goog-api-key", HeaderValue::from_static("nyro-key"));
        headers.insert(
            "proxy-authorization",
            HeaderValue::from_static("Basic secret"),
        );
        headers.insert("cookie", HeaderValue::from_static("session=secret"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("10.0.0.1"));
        headers.insert("x-real-ip", HeaderValue::from_static("10.0.0.2"));
        headers.insert("remote-host", HeaderValue::from_static("client.local"));
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("anthropic-beta", HeaderValue::from_static("prompt-caching"));

        let forwarded = forwarded_client_headers(&headers);

        assert!(forwarded.get("authorization").is_none());
        assert!(forwarded.get("x-api-key").is_none());
        assert!(forwarded.get("x-goog-api-key").is_none());
        assert!(forwarded.get("proxy-authorization").is_none());
        assert!(forwarded.get("cookie").is_none());
        assert!(forwarded.get("x-forwarded-for").is_none());
        assert!(forwarded.get("x-real-ip").is_none());
        assert!(forwarded.get("remote-host").is_none());
        assert!(forwarded.get("connection").is_none());
        assert_eq!(forwarded.get("anthropic-beta").unwrap(), "prompt-caching");
    }

    #[test]
    fn forwarded_client_headers_drop_client_encoding_negotiation() {
        let mut headers = HeaderMap::new();
        headers.insert("accept-encoding", HeaderValue::from_static("gzip"));

        let forwarded = forwarded_client_headers(&headers);

        assert!(
            forwarded.get("accept-encoding").is_none(),
            "reqwest must own upstream response decompression; client encoding hints are only for the Nyro response"
        );
    }
}

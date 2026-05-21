use super::*;

pub(super) fn flatten_route_cache_columns(
    cache: Option<&RouteCacheConfig>,
) -> (Option<i64>, Option<i64>, Option<f64>) {
    let Some(cache) = cache else {
        return (None, None, None);
    };
    let exact_ttl = cache.exact.as_ref().map(|exact| exact.ttl.unwrap_or(0));
    let semantic_ttl = cache
        .semantic
        .as_ref()
        .map(|semantic| semantic.ttl.unwrap_or(0));
    let semantic_threshold = cache
        .semantic
        .as_ref()
        .and_then(|semantic| semantic.threshold);
    (exact_ttl, semantic_ttl, semantic_threshold)
}

pub(super) fn resolve_route_cache(route: &Route) -> Option<RouteCacheConfig> {
    let exact = route.cache_exact_ttl.map(|ttl| RouteExactCacheConfig {
        ttl: if ttl > 0 { Some(ttl) } else { None },
    });
    let semantic = route
        .cache_semantic_ttl
        .map(|ttl| RouteSemanticCacheConfig {
            ttl: if ttl > 0 { Some(ttl) } else { None },
            threshold: route.cache_semantic_threshold,
        });
    if exact.is_none() && semantic.is_none() {
        None
    } else {
        Some(RouteCacheConfig { exact, semantic })
    }
}
pub(super) fn ensure_virtual_model(model: &str) -> anyhow::Result<()> {
    if model.trim().is_empty() {
        anyhow::bail!("virtual_model cannot be empty");
    }
    Ok(())
}

pub(super) fn normalize_route_strategy(strategy: Option<&str>) -> anyhow::Result<String> {
    let normalized = strategy.unwrap_or("weighted").trim().to_ascii_lowercase();
    match normalized.as_str() {
        "weighted" | "priority" => Ok(normalized),
        _ => anyhow::bail!("unsupported route strategy: {normalized}"),
    }
}

pub(super) fn normalize_create_route_targets(
    input: &CreateRoute,
) -> anyhow::Result<Vec<CreateRouteTarget>> {
    if !input.targets.is_empty() {
        return Ok(input.targets.clone());
    }
    if !input.target_provider.trim().is_empty() && !input.target_model.trim().is_empty() {
        return Ok(vec![CreateRouteTarget {
            provider_id: input.target_provider.clone(),
            model: input.target_model.clone(),
            weight: Some(100),
            priority: Some(1),
        }]);
    }
    anyhow::bail!("at least one route target is required")
}

pub(super) fn normalize_update_route_targets(
    current: &Route,
    input: &UpdateRoute,
) -> anyhow::Result<Vec<CreateRouteTarget>> {
    if let Some(targets) = &input.targets {
        let mapped = targets
            .iter()
            .map(|target| CreateRouteTarget {
                provider_id: target.provider_id.clone(),
                model: target.model.clone(),
                weight: target.weight,
                priority: target.priority,
            })
            .collect();
        return Ok(mapped);
    }

    let provider = input
        .target_provider
        .clone()
        .unwrap_or_else(|| current.target_provider.clone());
    let model = input
        .target_model
        .clone()
        .unwrap_or_else(|| current.target_model.clone());
    if provider.trim().is_empty() || model.trim().is_empty() {
        anyhow::bail!("route target cannot be empty");
    }
    Ok(vec![CreateRouteTarget {
        provider_id: provider,
        model,
        weight: Some(100),
        priority: Some(1),
    }])
}

pub(super) fn ensure_route_targets_valid(targets: &[CreateRouteTarget]) -> anyhow::Result<()> {
    if targets.is_empty() {
        anyhow::bail!("at least one route target is required");
    }
    for target in targets {
        if target.provider_id.trim().is_empty() {
            anyhow::bail!("target provider_id cannot be empty");
        }
        if target.model.trim().is_empty() {
            anyhow::bail!("target model cannot be empty");
        }
        let weight = target.weight.unwrap_or(100);
        if weight < 0 {
            anyhow::bail!("target weight must be >= 0");
        }
        let priority = target.priority.unwrap_or(1);
        if !(1..=2).contains(&priority) {
            anyhow::bail!("target priority must be 1 or 2");
        }
    }
    Ok(())
}

use super::*;

impl AdminService {
    // ── Settings ──

    pub async fn get_setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.gw.storage.settings().get(key).await
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.gw.storage.settings().set(key, value).await
    }

    pub async fn get_cache_settings(&self) -> anyhow::Result<serde_json::Value> {
        let runtime = self.gw.effective_cache_config();
        Ok(runtime.to_admin_json())
    }

    pub async fn update_cache_settings(&self, input: serde_json::Value) -> anyhow::Result<()> {
        let parsed = crate::cache::CacheConfig::from_admin_json(&input)
            .ok_or_else(|| anyhow::anyhow!("invalid cache settings payload"))?;
        self.gw.reload_cache_runtime(parsed.clone()).await?;
        let raw = serde_json::to_string(&parsed.to_admin_json())?;
        self.gw.storage.settings().set("cache_settings", &raw).await
    }

    pub async fn flush_cache(&self) -> anyhow::Result<()> {
        let cache_backend = (**self.gw.cache_backend.load()).clone();
        if let Some(cache) = cache_backend {
            cache.flush().await?;
        }
        Ok(())
    }

    pub async fn delete_cache_key(&self, key: &str) -> anyhow::Result<()> {
        let cache_backend = (**self.gw.cache_backend.load()).clone();
        if let Some(cache) = cache_backend {
            cache.delete(key).await?;
        }
        Ok(())
    }

    pub async fn get_cache_stats(&self) -> anyhow::Result<serde_json::Value> {
        let runtime = self.gw.effective_cache_config();
        let cache_backend = (**self.gw.cache_backend.load()).clone();
        let vector_store = (**self.gw.vector_store.load()).clone();
        let healthy = if let Some(cache) = cache_backend.as_ref() {
            cache.ping().await.unwrap_or(false)
        } else {
            false
        };
        Ok(serde_json::json!({
            "exact_enabled": runtime.exact.enabled,
            "semantic_enabled": runtime.semantic.enabled,
            "backend": cache_backend.as_ref().map(|b| b.backend_name()).unwrap_or("disabled"),
            "vector_store": if vector_store.is_some() { "memory" } else { "disabled" },
            "healthy": healthy,
            "singleflight_in_flight": self.gw.cache_in_flight.len(),
        }))
    }
}

use std::future::Future;
use std::sync::Arc;

use crate::error::CacheError;

pub struct CachedResult {
    pub etag: String,
    pub body: String,
}

#[derive(Clone)]
pub struct QueryCache {
    inner: moka::future::Cache<String, Arc<CachedResult>>,
}

impl QueryCache {
    pub fn new(max_bytes: u64) -> QueryCache {
        let inner = moka::future::Cache::builder()
            .max_capacity(max_bytes)
            .weigher(|_key: &String, value: &Arc<CachedResult>| {
                value.body.len().min(u32::MAX as usize) as u32
            })
            .build();
        QueryCache { inner }
    }

    pub async fn get(&self, key: &str) -> Option<Arc<CachedResult>> {
        let result = self.inner.get(key).await;
        match &result {
            Some(value) => log::info!(
                "event=query_cache_lookup result=hit key={} bytes={}",
                key,
                value.body.len(),
            ),
            None => log::info!("event=query_cache_lookup result=miss key={}", key),
        }
        result
    }

    pub async fn get_or_compute<F>(
        &self,
        key: String,
        compute: F,
    ) -> Result<Arc<CachedResult>, CacheError>
    where
        F: Future<Output = Result<String, CacheError>>,
    {
        if let Some(hit) = self.inner.get(&key).await {
            log::info!(
                "event=query_cache_get_or_compute result=hit key={} bytes={}",
                key,
                hit.body.len(),
            );
            return Ok(hit);
        }
        log::info!("event=query_cache_get_or_compute result=miss key={}", key);
        let body = compute.await?;
        let result = Arc::new(CachedResult {
            etag: key.clone(),
            body,
        });
        self.inner.insert(key, result.clone()).await;
        log::info!(
            "event=query_cache_store key={} bytes={}",
            result.etag,
            result.body.len(),
        );
        Ok(result)
    }
}

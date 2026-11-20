pub mod client;
pub mod launch;
pub mod pull;
pub mod push;

pub use client::{ApiError, Client};
pub use condensate_core as core;

/// Retry a fallible async operation a few times with a short backoff. Chunk transfers go over
/// home internet and tunnels, so the occasional dropped connection is expected.
pub(crate) async fn with_retries<T, F, Fut>(mut op: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut attempt = 0;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempt += 1;
                let permanent = e.downcast_ref::<ApiError>().is_some_and(|a| a.status < 500 && a.status != 429);
                if attempt >= 4 || permanent {
                    return Err(e);
                }
                tokio::time::sleep(std::time::Duration::from_millis(500 * (1 << attempt))).await;
            }
        }
    }
}

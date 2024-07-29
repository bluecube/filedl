use std::panic::resume_unwind;

use tokio::task::spawn_blocking;

/// Wrapper around spawn_blocking that is not cancelable and propagates panic.
pub async fn simple_spawn_blocking<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let join_result = spawn_blocking(f).await;

    match join_result {
        Ok(r) => r,
        Err(e) => {
            if let Ok(reason) = e.try_into_panic() {
                resume_unwind(reason)
            } else {
                unreachable!("We never cancel the join handle.")
            }
        }
    }
}

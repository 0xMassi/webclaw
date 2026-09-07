//! Optional response-payload observation. Counts decoded bytes actually read,
//! including partial bodies and retries. This is NOT wire or invoice metering:
//! redirect bodies consumed internally, request bytes and protocol overhead are
//! not exposed by the HTTP transport. Callbacks must be quick and non-panicking.
use std::{future::Future, sync::Arc};

#[derive(Debug, Clone)]
pub struct Transfer {
    pub host: String,
    /// Authority only; credentials and URL paths are never included.
    pub proxy: Option<String>,
    pub decoded_bytes: u64,
    pub status: Option<u16>,
    pub complete: bool,
}

pub type Observer = Arc<dyn Fn(Transfer) + Send + Sync>;

tokio::task_local! { static OBSERVER: Option<Observer>; }

pub async fn observe<F: Future>(observer: Option<Observer>, future: F) -> F::Output {
    OBSERVER.scope(observer, future).await
}

/// Spawn work with the current observer, so background batches/crawls retain
/// attribution after the initiating request has completed.
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let observer = OBSERVER.try_with(Clone::clone).ok().flatten();
    tokio::spawn(observe(observer, future))
}

pub fn record(transfer: Transfer) {
    let _ = OBSERVER.try_with(|observer| {
        if let Some(observer) = observer {
            observer(transfer);
        }
    });
}

pub(crate) fn authority(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    Some(match url.port() {
        Some(port) => format!("{}:{port}", url.host()?),
        None => url.host()?.to_string(),
    })
}

pub(crate) struct Attempt {
    observer: Option<Observer>,
    pub transfer: Transfer,
}

impl Attempt {
    pub fn new(url: &str, proxy: Option<&str>) -> Self {
        Self {
            observer: OBSERVER.try_with(Clone::clone).ok().flatten(),
            transfer: Transfer {
                host: authority(url).unwrap_or_default(),
                proxy: proxy.map(str::to_owned),
                decoded_bytes: 0,
                status: None,
                complete: false,
            },
        }
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if let Some(observer) = self.observer.take() {
            observer(self.transfer.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn cancellation_and_spawn_keep_separate_request_attribution() {
        let a = Arc::new(Mutex::new(Vec::new()));
        let b = Arc::new(Mutex::new(Vec::new()));
        let run = |sink: Arc<Mutex<Vec<Transfer>>>, bytes| async move {
            let observer: Observer = Arc::new(move |t| sink.lock().unwrap().push(t));
            observe(Some(observer), async move {
                spawn(async move {
                    let mut attempt = Attempt::new(
                        "https://user:secret@example.com/private?key=secret",
                        Some("proxy.example:123"),
                    );
                    attempt.transfer.decoded_bytes = bytes;
                    tokio::task::yield_now().await;
                    // Partial/error return drops the attempt and retains bytes.
                })
                .await
                .unwrap();
            })
            .await;
        };
        tokio::join!(run(a.clone(), 123), run(b.clone(), 456));
        assert_eq!(a.lock().unwrap()[0].decoded_bytes, 123);
        assert_eq!(b.lock().unwrap()[0].decoded_bytes, 456);
        assert_eq!(a.lock().unwrap()[0].host, "example.com");
        assert!(!a.lock().unwrap()[0].complete);
        assert_eq!(
            authority("http://user:password@proxy.example:123"),
            Some("proxy.example:123".into())
        );
    }
}

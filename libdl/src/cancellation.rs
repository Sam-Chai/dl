//! Cooperative cancellation shared between the caller and the downloader.
use tokio::sync::watch;

#[derive(Debug, Clone)]
pub struct DownloadCancellation(watch::Sender<bool>);

impl Default for DownloadCancellation {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

impl DownloadCancellation {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        loop {
            if *receiver.borrow_and_update() {
                return;
            }
            if receiver.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

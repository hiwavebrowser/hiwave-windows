//! Download management with progress tracking.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rustkit_http::Client as HttpClient;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, trace};

use crate::{NetError, Request};

/// Unique identifier for a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DownloadId(u64);

impl DownloadId {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl Default for DownloadId {
    fn default() -> Self {
        Self::new()
    }
}

/// Download state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// Download is pending.
    Pending,
    /// Download is in progress.
    InProgress,
    /// Download is paused.
    Paused,
    /// Download completed successfully.
    Completed,
    /// Download failed.
    Failed,
    /// Download was cancelled.
    Cancelled,
}

/// Download progress information.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed_bps: f64,
}

impl DownloadProgress {
    /// Get progress as a percentage (0.0 - 1.0).
    pub fn percentage(&self) -> Option<f64> {
        self.total.map(|t| self.downloaded as f64 / t as f64)
    }
}

/// Download event.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// Download started.
    Started {
        id: DownloadId,
        url: String,
        filename: String,
    },
    /// Progress update.
    Progress {
        id: DownloadId,
        progress: DownloadProgress,
    },
    /// Download completed.
    Completed { id: DownloadId, path: PathBuf },
    /// Download failed.
    Failed { id: DownloadId, error: String },
    /// Download cancelled.
    Cancelled { id: DownloadId },
    /// Download paused.
    Paused { id: DownloadId },
    /// Download resumed.
    Resumed { id: DownloadId },
}

/// Download metadata.
#[derive(Debug)]
pub struct Download {
    pub id: DownloadId,
    pub url: String,
    pub destination: PathBuf,
    pub filename: String,
    pub state: DownloadState,
    pub progress: DownloadProgress,
    pub mime_type: Option<String>,
    cancel_tx: Option<mpsc::Sender<()>>,
}

impl Download {
    fn new(id: DownloadId, url: String, destination: PathBuf) -> Self {
        let filename = destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download")
            .to_string();

        Self {
            id,
            url,
            destination,
            filename,
            state: DownloadState::Pending,
            progress: DownloadProgress {
                downloaded: 0,
                total: None,
                speed_bps: 0.0,
            },
            mime_type: None,
            cancel_tx: None,
        }
    }
}

/// Download manager.
pub struct DownloadManager {
    downloads: RwLock<HashMap<DownloadId, Download>>,
    event_tx: RwLock<Option<mpsc::UnboundedSender<DownloadEvent>>>,
}

impl DownloadManager {
    /// Create a new download manager.
    pub fn new() -> Self {
        Self {
            downloads: RwLock::new(HashMap::new()),
            event_tx: RwLock::new(None),
        }
    }

    /// Set the event sender.
    pub async fn set_event_sender(&self, tx: mpsc::UnboundedSender<DownloadEvent>) {
        *self.event_tx.write().await = Some(tx);
    }

    /// Emit an event.
    async fn emit(&self, event: DownloadEvent) {
        if let Some(tx) = self.event_tx.read().await.as_ref() {
            let _ = tx.send(event);
        }
    }

    /// Start a download.
    pub async fn start(
        &self,
        request: Request,
        destination: PathBuf,
        _client: &HttpClient,
    ) -> Result<DownloadId, NetError> {
        let id = DownloadId::new();
        let url = request.url.to_string();

        info!(id = id.raw(), url = %url, "Starting download");

        // Create download entry
        let mut download = Download::new(id, url.clone(), destination.clone());
        download.state = DownloadState::InProgress;

        // Create cancellation channel
        let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);
        download.cancel_tx = Some(cancel_tx);

        self.downloads.write().await.insert(id, download);

        // Emit started event
        let filename = destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download")
            .to_string();
        self.emit(DownloadEvent::Started {
            id,
            url: url.clone(),
            filename,
        })
        .await;

        // Spawn download task
        let _downloads = Arc::new(RwLock::new(HashMap::<DownloadId, Download>::new()));
        let event_tx = self.event_tx.read().await.clone();

        // For downloads, we use the streaming API
        let url_str = request.url.to_string();
        tokio::spawn(async move {
            let result = Self::download_file_streaming(
                id,
                &url_str,
                destination.clone(),
                &mut cancel_rx,
                event_tx.as_ref(),
            )
            .await;

            match result {
                Ok(()) => {
                    if let Some(tx) = event_tx.as_ref() {
                        let _ = tx.send(DownloadEvent::Completed {
                            id,
                            path: destination,
                        });
                    }
                }
                Err(NetError::Cancelled) => {
                    if let Some(tx) = event_tx.as_ref() {
                        let _ = tx.send(DownloadEvent::Cancelled { id });
                    }
                }
                Err(e) => {
                    error!(id = id.raw(), error = %e, "Download failed");
                    if let Some(tx) = event_tx.as_ref() {
                        let _ = tx.send(DownloadEvent::Failed {
                            id,
                            error: e.to_string(),
                        });
                    }
                }
            }
        });

        Ok(id)
    }

    /// Internal download implementation using rustkit-http streaming.
    async fn download_file_streaming(
        id: DownloadId,
        url: &str,
        destination: PathBuf,
        cancel_rx: &mut mpsc::Receiver<()>,
        event_tx: Option<&mpsc::UnboundedSender<DownloadEvent>>,
    ) -> Result<(), NetError> {
        // Create a new client for this download (streaming requires ownership)
        let client = HttpClient::new().map_err(|e| NetError::RequestFailed(e.to_string()))?;

        // Start streaming request
        let mut response = client
            .get_streaming(url)
            .await
            .map_err(|e| NetError::RequestFailed(e.to_string()))?;

        let total_size = response.content_length;

        // Create parent directories
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Create file
        let mut file = File::create(&destination).await?;
        let mut downloaded: u64 = 0;
        let mut buf = vec![0u8; 8192]; // 8KB buffer

        let start_time = std::time::Instant::now();

        loop {
            // Check for cancellation
            if cancel_rx.try_recv().is_ok() {
                debug!(id = id.raw(), "Download cancelled");
                return Err(NetError::Cancelled);
            }

            let n = response
                .chunk(&mut buf)
                .await
                .map_err(|e| NetError::RequestFailed(e.to_string()))?;

            if n == 0 {
                break;
            }

            file.write_all(&buf[..n]).await?;
            downloaded += n as u64;

            // Calculate speed
            let elapsed = start_time.elapsed().as_secs_f64();
            let speed_bps = if elapsed > 0.0 {
                downloaded as f64 / elapsed
            } else {
                0.0
            };

            // Emit progress (throttled - every 100KB or so)
            if downloaded % (100 * 1024) < n as u64 {
                if let Some(tx) = event_tx {
                    let _ = tx.send(DownloadEvent::Progress {
                        id,
                        progress: DownloadProgress {
                            downloaded,
                            total: total_size,
                            speed_bps,
                        },
                    });
                }
            }

            trace!(id = id.raw(), downloaded, total = ?total_size, "Download progress");
        }

        file.flush().await?;

        info!(id = id.raw(), bytes = downloaded, "Download completed");
        Ok(())
    }

    /// Cancel a download.
    pub async fn cancel(&self, id: DownloadId) -> Result<(), NetError> {
        let mut downloads = self.downloads.write().await;
        if let Some(download) = downloads.get_mut(&id) {
            if let Some(tx) = download.cancel_tx.take() {
                let _ = tx.send(()).await;
            }
            download.state = DownloadState::Cancelled;
            Ok(())
        } else {
            Err(NetError::RequestFailed("Download not found".into()))
        }
    }

    /// Get download state.
    pub async fn get_state(&self, id: DownloadId) -> Option<DownloadState> {
        self.downloads.read().await.get(&id).map(|d| d.state)
    }

    /// Get download progress.
    pub async fn get_progress(&self, id: DownloadId) -> Option<DownloadProgress> {
        self.downloads
            .read()
            .await
            .get(&id)
            .map(|d| d.progress.clone())
    }

    /// List all downloads.
    pub async fn list(&self) -> Vec<(DownloadId, DownloadState, String)> {
        self.downloads
            .read()
            .await
            .iter()
            .map(|(id, d)| (*id, d.state, d.filename.clone()))
            .collect()
    }

    /// Remove completed/failed/cancelled downloads.
    pub async fn cleanup(&self) {
        let mut downloads = self.downloads.write().await;
        downloads.retain(|_, d| {
            !matches!(
                d.state,
                DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
            )
        });
    }
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_id_uniqueness() {
        let id1 = DownloadId::new();
        let id2 = DownloadId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_download_progress_percentage() {
        let progress = DownloadProgress {
            downloaded: 50,
            total: Some(100),
            speed_bps: 1000.0,
        };
        assert_eq!(progress.percentage(), Some(0.5));

        let progress_no_total = DownloadProgress {
            downloaded: 50,
            total: None,
            speed_bps: 1000.0,
        };
        assert_eq!(progress_no_total.percentage(), None);
    }

    #[test]
    fn test_download_states() {
        assert_eq!(DownloadState::Pending, DownloadState::Pending);
        assert_ne!(DownloadState::Pending, DownloadState::InProgress);
    }

    #[tokio::test]
    async fn test_download_manager_creation() {
        let manager = DownloadManager::new();
        let list = manager.list().await;
        assert!(list.is_empty());
    }
}

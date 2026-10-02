//! # RustKit Web Workers
//!
//! Web Workers implementation for the RustKit browser engine.
//!
//! ## Features
//!
//! - **DedicatedWorker**: `new Worker(url)`
//! - **SharedWorker**: `new SharedWorker(url)`
//! - **MessageChannel**: Bidirectional messaging
//! - **Transferable objects**: ArrayBuffer transfer
//! - **Worker lifecycle**: terminate, error handling
//!
//! ## Architecture
//!
//! ```text
//! Main Thread
//!     │
//!     ├── DedicatedWorker ──── postMessage ───→ Worker Thread
//!     │       └── MessagePort
//!     │
//!     └── SharedWorker ─────── connect ──────→ Shared Thread
//!             └── port (MessagePort)
//!
//! MessageChannel
//!     ├── port1 ◄──────────────────────►  port2
//!     └── (structured clone / transfer)
//! ```

use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, RwLock};
use url::Url;

// ==================== Errors ====================

/// Web Worker errors.
#[derive(Error, Debug, Clone)]
pub enum WorkerError {
    #[error("Script error: {0}")]
    ScriptError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Security error: {0}")]
    SecurityError(String),

    #[error("Data clone error: {0}")]
    DataCloneError(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Terminated")]
    Terminated,
}

// ==================== Types ====================

/// Unique identifier for a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkerId(u64);

impl WorkerId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Unique identifier for a message port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortId(u64);

impl PortId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Worker type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerType {
    Classic,
    Module,
}

impl Default for WorkerType {
    fn default() -> Self {
        Self::Classic
    }
}

/// Worker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// Worker is being created.
    Pending,
    /// Worker is running.
    Running,
    /// Worker has been terminated.
    Terminated,
    /// Worker encountered an error.
    Errored,
}

impl Default for WorkerState {
    fn default() -> Self {
        Self::Pending
    }
}

// ==================== Structured Clone ====================

/// A message that can be sent between workers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerMessage {
    /// Message data (JSON serializable).
    pub data: JsonValue,
    
    /// Transferred port IDs.
    pub transfer_ports: Vec<PortId>,
    
    /// Transferred array buffer IDs.
    pub transfer_buffers: Vec<u64>,
}

impl WorkerMessage {
    /// Create a new message.
    pub fn new(data: JsonValue) -> Self {
        Self {
            data,
            transfer_ports: Vec::new(),
            transfer_buffers: Vec::new(),
        }
    }

    /// Create with transfers.
    pub fn with_transfers(data: JsonValue, ports: Vec<PortId>, buffers: Vec<u64>) -> Self {
        Self {
            data,
            transfer_ports: ports,
            transfer_buffers: buffers,
        }
    }
}

// ==================== Transferable ====================

/// A transferable object (ArrayBuffer).
#[derive(Debug)]
pub struct TransferableBuffer {
    pub id: u64,
    pub data: Vec<u8>,
    pub detached: bool,
}

impl TransferableBuffer {
    /// Create a new buffer.
    pub fn new(data: Vec<u8>) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self {
            id: COUNTER.fetch_add(1, Ordering::Relaxed),
            data,
            detached: false,
        }
    }

    /// Transfer ownership (detaches the buffer).
    pub fn transfer(&mut self) -> Option<Vec<u8>> {
        if self.detached {
            None
        } else {
            self.detached = true;
            Some(std::mem::take(&mut self.data))
        }
    }

    /// Check if detached.
    pub fn is_detached(&self) -> bool {
        self.detached
    }

    /// Get byte length.
    pub fn byte_length(&self) -> usize {
        if self.detached {
            0
        } else {
            self.data.len()
        }
    }
}

// ==================== MessagePort ====================

/// A message port for bidirectional communication.
#[derive(Debug)]
pub struct MessagePort {
    /// Port ID.
    pub id: PortId,
    
    /// Message sender.
    tx: mpsc::UnboundedSender<WorkerMessage>,
    
    /// Message receiver.
    rx: Option<mpsc::UnboundedReceiver<WorkerMessage>>,
    
    /// Whether started (messages flow).
    started: bool,
    
    /// Whether closed.
    closed: bool,

    /// Entangled port ID.
    #[allow(dead_code)]
    entangled_port: Option<PortId>,
}

impl MessagePort {
    /// Create a new port pair.
    pub fn create_pair() -> (Self, Self) {
        let (tx1, rx1) = mpsc::unbounded_channel();
        let (tx2, rx2) = mpsc::unbounded_channel();
        
        let id1 = PortId::new();
        let id2 = PortId::new();
        
        let port1 = Self {
            id: id1,
            tx: tx2,
            rx: Some(rx1),
            started: false,
            closed: false,
            entangled_port: Some(id2),
        };
        
        let port2 = Self {
            id: id2,
            tx: tx1,
            rx: Some(rx2),
            started: false,
            closed: false,
            entangled_port: Some(id1),
        };
        
        (port1, port2)
    }

    /// Post a message.
    pub fn post_message(&self, message: WorkerMessage) -> Result<(), WorkerError> {
        if self.closed {
            return Err(WorkerError::InvalidState("Port is closed".to_string()));
        }
        
        self.tx.send(message).map_err(|_| {
            WorkerError::InvalidState("Entangled port is closed".to_string())
        })
    }

    /// Receive a message (non-blocking).
    pub fn try_receive(&mut self) -> Option<WorkerMessage> {
        if !self.started || self.closed {
            return None;
        }
        
        self.rx.as_mut()?.try_recv().ok()
    }

    /// Start the port (enable message flow).
    pub fn start(&mut self) {
        self.started = true;
    }

    /// Close the port.
    pub fn close(&mut self) {
        self.closed = true;
        self.rx = None;
    }

    /// Check if closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

// ==================== MessageChannel ====================

/// A message channel with two entangled ports.
#[derive(Debug)]
pub struct MessageChannel {
    pub port1: MessagePort,
    pub port2: MessagePort,
}

impl MessageChannel {
    /// Create a new message channel.
    pub fn new() -> Self {
        let (port1, port2) = MessagePort::create_pair();
        Self { port1, port2 }
    }
}

impl Default for MessageChannel {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== DedicatedWorker ====================

/// A dedicated worker.
pub struct DedicatedWorker {
    /// Worker ID.
    pub id: WorkerId,
    
    /// Script URL.
    pub script_url: Url,
    
    /// Worker type.
    pub worker_type: WorkerType,
    
    /// Worker state.
    pub state: WorkerState,
    
    /// Worker name.
    pub name: String,
    
    /// Message port to worker.
    port: MessagePort,
    
    /// Error message.
    pub error: Option<String>,
    
    /// Terminate signal.
    terminate_tx: Option<oneshot::Sender<()>>,
}

impl DedicatedWorker {
    /// Create a new dedicated worker.
    pub fn new(script_url: Url, options: WorkerOptions) -> (Self, MessagePort) {
        let (port1, port2) = MessagePort::create_pair();
        let (terminate_tx, _terminate_rx) = oneshot::channel();
        
        let worker = Self {
            id: WorkerId::new(),
            script_url,
            worker_type: options.worker_type,
            state: WorkerState::Pending,
            name: options.name,
            port: port1,
            error: None,
            terminate_tx: Some(terminate_tx),
        };
        
        // port2 goes to the worker thread
        (worker, port2)
    }

    /// Post a message to the worker.
    pub fn post_message(&self, data: JsonValue) -> Result<(), WorkerError> {
        if self.state == WorkerState::Terminated {
            return Err(WorkerError::Terminated);
        }
        
        self.port.post_message(WorkerMessage::new(data))
    }

    /// Post a message with transfers.
    pub fn post_message_with_transfer(
        &self,
        data: JsonValue,
        transfer: Vec<TransferableBuffer>,
    ) -> Result<(), WorkerError> {
        if self.state == WorkerState::Terminated {
            return Err(WorkerError::Terminated);
        }
        
        let buffer_ids: Vec<u64> = transfer.iter().map(|b| b.id).collect();
        let message = WorkerMessage::with_transfers(data, Vec::new(), buffer_ids);
        self.port.post_message(message)
    }

    /// Terminate the worker.
    pub fn terminate(&mut self) {
        if let Some(tx) = self.terminate_tx.take() {
            let _ = tx.send(());
        }
        self.state = WorkerState::Terminated;
        self.port.close();
    }

    /// Set worker as running.
    pub fn set_running(&mut self) {
        self.state = WorkerState::Running;
        self.port.start();
    }

    /// Set worker error.
    pub fn set_error(&mut self, error: &str) {
        self.state = WorkerState::Errored;
        self.error = Some(error.to_string());
    }

    /// Receive message from worker.
    pub fn receive(&mut self) -> Option<WorkerMessage> {
        self.port.try_receive()
    }
}

/// Options for creating a worker.
#[derive(Debug, Clone, Default)]
pub struct WorkerOptions {
    pub worker_type: WorkerType,
    pub name: String,
    pub credentials: String,
}

// ==================== SharedWorker ====================

/// A shared worker.
pub struct SharedWorker {
    /// Worker ID.
    pub id: WorkerId,
    
    /// Script URL.
    pub script_url: Url,
    
    /// Worker type.
    pub worker_type: WorkerType,
    
    /// Worker state.
    pub state: WorkerState,
    
    /// Worker name.
    pub name: String,
    
    /// Port for communication.
    pub port: MessagePort,
    
    /// Connected port IDs.
    connected_ports: Vec<PortId>,
    
    /// Error message.
    pub error: Option<String>,
}

impl SharedWorker {
    /// Create a new shared worker.
    pub fn new(script_url: Url, options: WorkerOptions) -> (Self, MessagePort) {
        let (port1, port2) = MessagePort::create_pair();
        
        let worker = Self {
            id: WorkerId::new(),
            script_url,
            worker_type: options.worker_type,
            state: WorkerState::Pending,
            name: options.name,
            port: port1,
            connected_ports: vec![],
            error: None,
        };
        
        (worker, port2)
    }

    /// Connect a new port.
    pub fn connect(&mut self) -> MessagePort {
        let (port1, port2) = MessagePort::create_pair();
        self.connected_ports.push(port1.id);
        // port1 stays with worker, port2 goes to caller
        port2
    }

    /// Set worker as running.
    pub fn set_running(&mut self) {
        self.state = WorkerState::Running;
        self.port.start();
    }

    /// Set worker error.
    pub fn set_error(&mut self, error: &str) {
        self.state = WorkerState::Errored;
        self.error = Some(error.to_string());
    }
}

// ==================== Worker Manager ====================

/// Worker events.
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// Worker started.
    Started { id: WorkerId },
    /// Message received from worker.
    Message { id: WorkerId, message: JsonValue },
    /// Worker error.
    Error { id: WorkerId, message: String, filename: String, lineno: u32 },
    /// Worker terminated.
    Terminated { id: WorkerId },
}

/// Manages all workers.
pub struct WorkerManager {
    /// Dedicated workers.
    dedicated: Arc<RwLock<HashMap<WorkerId, DedicatedWorker>>>,
    
    /// Shared workers (by name + origin).
    shared: Arc<RwLock<HashMap<String, SharedWorker>>>,
    
    /// Event sender.
    event_tx: mpsc::UnboundedSender<WorkerEvent>,
}

impl WorkerManager {
    /// Create a new worker manager.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<WorkerEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        
        (Self {
            dedicated: Arc::new(RwLock::new(HashMap::new())),
            shared: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }, event_rx)
    }

    /// Create a dedicated worker.
    pub async fn create_dedicated(
        &self,
        script_url: &str,
        options: WorkerOptions,
    ) -> Result<WorkerId, WorkerError> {
        let url = Url::parse(script_url)
            .map_err(|e| WorkerError::NetworkError(e.to_string()))?;
        
        let (mut worker, _worker_port) = DedicatedWorker::new(url, options);
        let id = worker.id;
        
        // Simulate worker startup
        worker.set_running();
        
        let _ = self.event_tx.send(WorkerEvent::Started { id });
        
        self.dedicated.write().await.insert(id, worker);
        
        Ok(id)
    }

    /// Get a dedicated worker.
    pub async fn get_dedicated(&self, id: WorkerId) -> Option<()> {
        self.dedicated.read().await.get(&id).map(|_| ())
    }

    /// Post message to dedicated worker.
    pub async fn post_message_dedicated(
        &self,
        id: WorkerId,
        data: JsonValue,
    ) -> Result<(), WorkerError> {
        let workers = self.dedicated.read().await;
        let worker = workers.get(&id).ok_or(WorkerError::InvalidState(
            "Worker not found".to_string(),
        ))?;
        worker.post_message(data)
    }

    /// Terminate a dedicated worker.
    pub async fn terminate_dedicated(&self, id: WorkerId) -> Result<(), WorkerError> {
        let mut workers = self.dedicated.write().await;
        let worker = workers.get_mut(&id).ok_or(WorkerError::InvalidState(
            "Worker not found".to_string(),
        ))?;
        worker.terminate();
        
        let _ = self.event_tx.send(WorkerEvent::Terminated { id });
        
        Ok(())
    }

    /// Create or get a shared worker.
    pub async fn get_or_create_shared(
        &self,
        script_url: &str,
        options: WorkerOptions,
    ) -> Result<(WorkerId, MessagePort), WorkerError> {
        let url = Url::parse(script_url)
            .map_err(|e| WorkerError::NetworkError(e.to_string()))?;
        
        let key = format!("{}:{}", url.origin().ascii_serialization(), options.name);
        
        let mut shared = self.shared.write().await;
        
        if let Some(worker) = shared.get_mut(&key) {
            let port = worker.connect();
            return Ok((worker.id, port));
        }
        
        let (mut worker, _worker_port) = SharedWorker::new(url, options);
        let id = worker.id;
        worker.set_running();
        
        let port = worker.connect();
        
        let _ = self.event_tx.send(WorkerEvent::Started { id });
        
        shared.insert(key, worker);
        
        Ok((id, port))
    }

    /// Poll for messages from workers.
    pub async fn poll_messages(&self) -> Vec<(WorkerId, WorkerMessage)> {
        let mut messages = Vec::new();
        
        let mut workers = self.dedicated.write().await;
        for (id, worker) in workers.iter_mut() {
            while let Some(msg) = worker.receive() {
                messages.push((*id, msg));
            }
        }
        
        messages
    }

    /// Remove terminated workers.
    pub async fn cleanup(&self) {
        let mut workers = self.dedicated.write().await;
        workers.retain(|_, w| w.state != WorkerState::Terminated);
    }
}

impl Default for WorkerManager {
    fn default() -> Self {
        Self::new().0
    }
}

// ==================== WorkerGlobalScope ====================

/// Worker global scope (self inside worker).
#[derive(Debug)]
pub struct WorkerGlobalScope {
    /// Worker name.
    pub name: String,
    
    /// Location (script URL).
    pub location: Url,
    
    /// Self reference.
    pub worker_type: WorkerType,
    
    /// Navigator.
    pub navigator: WorkerNavigator,
    
    /// Port for communication (DedicatedWorkerGlobalScope).
    port: Option<MessagePort>,
    
    /// Ports for SharedWorkerGlobalScope.
    ports: Vec<MessagePort>,
    
    /// Close requested.
    close_requested: bool,
}

impl WorkerGlobalScope {
    /// Create for dedicated worker.
    pub fn new_dedicated(name: &str, location: Url, port: MessagePort) -> Self {
        Self {
            name: name.to_string(),
            location,
            worker_type: WorkerType::Classic,
            navigator: WorkerNavigator::new(),
            port: Some(port),
            ports: Vec::new(),
            close_requested: false,
        }
    }

    /// Create for shared worker.
    pub fn new_shared(name: &str, location: Url) -> Self {
        Self {
            name: name.to_string(),
            location,
            worker_type: WorkerType::Classic,
            navigator: WorkerNavigator::new(),
            port: None,
            ports: Vec::new(),
            close_requested: false,
        }
    }

    /// Post message (for dedicated worker).
    pub fn post_message(&self, data: JsonValue) -> Result<(), WorkerError> {
        let port = self.port.as_ref().ok_or(WorkerError::InvalidState(
            "Not a dedicated worker".to_string(),
        ))?;
        port.post_message(WorkerMessage::new(data))
    }

    /// Close the worker.
    pub fn close(&mut self) {
        self.close_requested = true;
    }

    /// Import scripts (stub).
    pub fn import_scripts(&self, _urls: &[&str]) -> Result<(), WorkerError> {
        // Would fetch and execute scripts
        Ok(())
    }

    /// Add port (for shared worker connect event).
    pub fn add_port(&mut self, port: MessagePort) {
        self.ports.push(port);
    }
}

/// Worker navigator.
#[derive(Debug, Clone)]
pub struct WorkerNavigator {
    pub user_agent: String,
    pub app_name: String,
    pub app_version: String,
    pub platform: String,
    pub language: String,
    pub languages: Vec<String>,
    pub online: bool,
    pub hardware_concurrency: usize,
}

impl WorkerNavigator {
    pub fn new() -> Self {
        Self {
            user_agent: "RustKit/1.0".to_string(),
            app_name: "RustKit".to_string(),
            app_version: "1.0".to_string(),
            platform: "Win32".to_string(),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
            online: true,
            hardware_concurrency: num_cpus(),
        }
    }
}

impl Default for WorkerNavigator {
    fn default() -> Self {
        Self::new()
    }
}

/// Get number of CPUs.
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_port_pair() {
        let (mut port1, mut port2) = MessagePort::create_pair();
        
        port1.start();
        port2.start();
        
        let msg = WorkerMessage::new(serde_json::json!({"hello": "world"}));
        port1.post_message(msg).unwrap();
        
        let received = port2.try_receive().unwrap();
        assert_eq!(received.data["hello"], "world");
    }

    #[test]
    fn test_message_channel() {
        let mut channel = MessageChannel::new();
        
        channel.port1.start();
        channel.port2.start();
        
        let msg = WorkerMessage::new(serde_json::json!(42));
        channel.port1.post_message(msg).unwrap();
        
        let received = channel.port2.try_receive().unwrap();
        assert_eq!(received.data, 42);
    }

    #[test]
    fn test_port_close() {
        let (mut port1, _port2) = MessagePort::create_pair();
        
        port1.close();
        assert!(port1.is_closed());
        
        let result = port1.post_message(WorkerMessage::new(serde_json::json!(null)));
        assert!(result.is_err());
    }

    #[test]
    fn test_transferable_buffer() {
        let mut buffer = TransferableBuffer::new(vec![1, 2, 3, 4]);
        
        assert_eq!(buffer.byte_length(), 4);
        assert!(!buffer.is_detached());
        
        let data = buffer.transfer().unwrap();
        assert_eq!(data, vec![1, 2, 3, 4]);
        assert!(buffer.is_detached());
        assert_eq!(buffer.byte_length(), 0);
        
        assert!(buffer.transfer().is_none());
    }

    #[test]
    fn test_dedicated_worker() {
        let url = Url::parse("https://example.com/worker.js").unwrap();
        let options = WorkerOptions::default();
        
        let (mut worker, _port) = DedicatedWorker::new(url.clone(), options);
        
        assert_eq!(worker.state, WorkerState::Pending);
        assert_eq!(worker.script_url, url);
        
        worker.set_running();
        assert_eq!(worker.state, WorkerState::Running);
    }

    #[test]
    fn test_worker_terminate() {
        let url = Url::parse("https://example.com/worker.js").unwrap();
        let (mut worker, _port) = DedicatedWorker::new(url, WorkerOptions::default());
        
        worker.set_running();
        worker.terminate();
        
        assert_eq!(worker.state, WorkerState::Terminated);
        assert!(worker.post_message(serde_json::json!(null)).is_err());
    }

    #[test]
    fn test_shared_worker() {
        let url = Url::parse("https://example.com/shared.js").unwrap();
        let options = WorkerOptions {
            name: "my-worker".to_string(),
            ..Default::default()
        };
        
        let (mut worker, _port) = SharedWorker::new(url, options);
        
        assert_eq!(worker.name, "my-worker");
        
        let _port2 = worker.connect();
        assert_eq!(worker.connected_ports.len(), 1);
    }

    #[test]
    fn test_worker_global_scope() {
        let url = Url::parse("https://example.com/worker.js").unwrap();
        let (port, _) = MessagePort::create_pair();
        
        let scope = WorkerGlobalScope::new_dedicated("test", url.clone(), port);
        
        assert_eq!(scope.name, "test");
        assert_eq!(scope.location, url);
    }

    #[test]
    fn test_worker_navigator() {
        let nav = WorkerNavigator::new();
        
        assert!(!nav.user_agent.is_empty());
        assert!(nav.online);
        assert!(nav.hardware_concurrency >= 1);
    }

    #[tokio::test]
    async fn test_worker_manager_create() {
        let (manager, _rx) = WorkerManager::new();
        
        let id = manager.create_dedicated(
            "https://example.com/worker.js",
            WorkerOptions::default(),
        ).await.unwrap();
        
        assert!(manager.get_dedicated(id).await.is_some());
    }

    #[tokio::test]
    async fn test_worker_manager_terminate() {
        let (manager, _rx) = WorkerManager::new();
        
        let id = manager.create_dedicated(
            "https://example.com/worker.js",
            WorkerOptions::default(),
        ).await.unwrap();
        
        manager.terminate_dedicated(id).await.unwrap();
        
        // Worker is still tracked but terminated
        manager.cleanup().await;
    }

    #[tokio::test]
    async fn test_shared_worker_reuse() {
        let (manager, _rx) = WorkerManager::new();
        
        let options = WorkerOptions {
            name: "shared".to_string(),
            ..Default::default()
        };
        
        let (id1, _port1) = manager.get_or_create_shared(
            "https://example.com/shared.js",
            options.clone(),
        ).await.unwrap();
        
        let (id2, _port2) = manager.get_or_create_shared(
            "https://example.com/shared.js",
            options,
        ).await.unwrap();
        
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_worker_message_with_transfer() {
        let buffer = TransferableBuffer::new(vec![1, 2, 3]);
        let msg = WorkerMessage::with_transfers(
            serde_json::json!({"type": "data"}),
            vec![],
            vec![buffer.id],
        );
        
        assert_eq!(msg.transfer_buffers.len(), 1);
    }
}


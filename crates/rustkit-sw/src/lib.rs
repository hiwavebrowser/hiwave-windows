//! # RustKit Service Workers
//!
//! Service Workers implementation for the RustKit browser engine.
//!
//! ## Features
//!
//! - **Registration**: `navigator.serviceWorker.register()`
//! - **Lifecycle**: install, activate, fetch events
//! - **Cache API**: `caches.open()`, `cache.add()`, `cache.match()`
//! - **Clients API**: Access to controlled pages
//! - **Fetch Interception**: Offline-first patterns
//!
//! ## Architecture
//!
//! ```text
//! ServiceWorkerContainer (navigator.serviceWorker)
//!     │
//!     └── ServiceWorkerRegistration
//!             ├── installing (ServiceWorker)
//!             ├── waiting (ServiceWorker)
//!             ├── active (ServiceWorker)
//!             └── scope
//!
//! CacheStorage (caches)
//!     └── Cache
//!             └── Request → Response
//! ```

use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use url::Url;

// ==================== Errors ====================

/// Errors that can occur in service worker operations.
#[derive(Error, Debug, Clone)]
pub enum ServiceWorkerError {
    #[error("Registration failed: {0}")]
    RegistrationFailed(String),

    #[error("Script error: {0}")]
    ScriptError(String),

    #[error("Security error: {0}")]
    SecurityError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("State error: {0}")]
    StateError(String),

    #[error("Cache error: {0}")]
    CacheError(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Not implemented: {0}")]
    NotImplemented(String),
}

// ==================== Types ====================

/// Unique identifier for a service worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceWorkerId(u64);

impl ServiceWorkerId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Service worker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceWorkerState {
    /// Initial state, script being parsed.
    Parsed,
    /// Installing (install event).
    Installing,
    /// Installed but waiting for activation.
    Installed,
    /// Activating (activate event).
    Activating,
    /// Active and controlling pages.
    Activated,
    /// Redundant (replaced or install failed).
    Redundant,
}

impl Default for ServiceWorkerState {
    fn default() -> Self {
        Self::Parsed
    }
}

/// Service worker update via cache type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateViaCache {
    #[default]
    Imports,
    All,
    None,
}

/// Service worker type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkerType {
    #[default]
    Classic,
    Module,
}

// ==================== Service Worker ====================

/// A service worker instance.
#[derive(Debug, Clone)]
pub struct ServiceWorker {
    /// Unique ID.
    pub id: ServiceWorkerId,
    
    /// Script URL.
    pub script_url: Url,
    
    /// Current state.
    pub state: ServiceWorkerState,
    
    /// Worker type.
    pub worker_type: WorkerType,
    
    /// Script content (loaded).
    pub script: Option<String>,
    
    /// Error message if failed.
    pub error: Option<String>,
    
    /// Time of last state change.
    pub state_changed_at: Instant,
}

impl ServiceWorker {
    /// Create a new service worker.
    pub fn new(script_url: Url, worker_type: WorkerType) -> Self {
        Self {
            id: ServiceWorkerId::new(),
            script_url,
            state: ServiceWorkerState::Parsed,
            worker_type,
            script: None,
            error: None,
            state_changed_at: Instant::now(),
        }
    }

    /// Set state.
    pub fn set_state(&mut self, state: ServiceWorkerState) {
        self.state = state;
        self.state_changed_at = Instant::now();
    }

    /// Check if active.
    pub fn is_active(&self) -> bool {
        self.state == ServiceWorkerState::Activated
    }

    /// Check if redundant.
    pub fn is_redundant(&self) -> bool {
        self.state == ServiceWorkerState::Redundant
    }

    /// Post message to worker.
    ///
    /// Status: Not implemented - requires worker context integration and message serialization.
    pub fn post_message(&self, _message: &str) -> Result<(), ServiceWorkerError> {
        if self.is_redundant() {
            return Err(ServiceWorkerError::StateError(
                "Cannot post message to redundant worker".to_string(),
            ));
        }
        Err(ServiceWorkerError::NotImplemented(
            "Service Worker message passing requires worker context integration".to_string(),
        ))
    }
}

// ==================== Registration Options ====================

/// Options for service worker registration.
#[derive(Debug, Clone, Default)]
pub struct RegistrationOptions {
    /// Scope URL.
    pub scope: Option<String>,
    
    /// Worker type.
    pub worker_type: WorkerType,
    
    /// Update via cache mode.
    pub update_via_cache: UpdateViaCache,
}

// ==================== Registration ====================

/// A service worker registration.
#[derive(Debug)]
pub struct ServiceWorkerRegistration {
    /// Scope URL.
    pub scope: Url,
    
    /// Installing worker.
    pub installing: Option<ServiceWorker>,
    
    /// Waiting worker (installed but not active).
    pub waiting: Option<ServiceWorker>,
    
    /// Active worker.
    pub active: Option<ServiceWorker>,
    
    /// Update via cache mode.
    pub update_via_cache: UpdateViaCache,
    
    /// Navigation preload enabled.
    pub navigation_preload_enabled: bool,
    
    /// Last update check time.
    pub last_update_check: Option<Instant>,
}

impl ServiceWorkerRegistration {
    /// Create a new registration.
    pub fn new(scope: Url, update_via_cache: UpdateViaCache) -> Self {
        Self {
            scope,
            installing: None,
            waiting: None,
            active: None,
            update_via_cache,
            navigation_preload_enabled: false,
            last_update_check: None,
        }
    }

    /// Get the active worker.
    pub fn get_active(&self) -> Option<&ServiceWorker> {
        self.active.as_ref()
    }

    /// Check if update is needed.
    pub fn needs_update(&self, check_interval: Duration) -> bool {
        match self.last_update_check {
            Some(last) => last.elapsed() > check_interval,
            None => true,
        }
    }

    /// Update the registration with a new worker.
    pub fn update(&mut self, script_url: Url, worker_type: WorkerType) {
        let worker = ServiceWorker::new(script_url, worker_type);
        self.installing = Some(worker);
        self.last_update_check = Some(Instant::now());
    }

    /// Transition installing to waiting.
    pub fn install_complete(&mut self) {
        if let Some(mut worker) = self.installing.take() {
            worker.set_state(ServiceWorkerState::Installed);
            self.waiting = Some(worker);
        }
    }

    /// Activate waiting worker.
    pub fn activate(&mut self) {
        if let Some(mut worker) = self.waiting.take() {
            worker.set_state(ServiceWorkerState::Activating);
            
            // Mark old active as redundant
            if let Some(mut old) = self.active.take() {
                old.set_state(ServiceWorkerState::Redundant);
            }
            
            worker.set_state(ServiceWorkerState::Activated);
            self.active = Some(worker);
        }
    }

    /// Skip waiting (force activate).
    pub fn skip_waiting(&mut self) {
        self.activate();
    }

    /// Unregister (mark as inactive).
    pub fn unregister(&mut self) {
        if let Some(mut worker) = self.active.take() {
            worker.set_state(ServiceWorkerState::Redundant);
        }
        if let Some(mut worker) = self.waiting.take() {
            worker.set_state(ServiceWorkerState::Redundant);
        }
        if let Some(mut worker) = self.installing.take() {
            worker.set_state(ServiceWorkerState::Redundant);
        }
    }
}

// ==================== Cache ====================

/// A cached request/response pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Request URL.
    pub url: String,
    
    /// Request method.
    pub method: String,
    
    /// Response status.
    pub status: u16,
    
    /// Response headers.
    pub headers: HashMap<String, String>,
    
    /// Response body.
    pub body: Vec<u8>,
    
    /// Cached at timestamp (ms since epoch).
    pub cached_at: u64,
}

/// A cache instance.
#[derive(Debug, Default)]
pub struct Cache {
    /// Cache name.
    pub name: String,
    
    /// Cached entries.
    entries: HashMap<String, CacheEntry>,
}

impl Cache {
    /// Create a new cache.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            entries: HashMap::new(),
        }
    }

    /// Match a request.
    pub fn match_request(&self, url: &str) -> Option<&CacheEntry> {
        self.entries.get(url)
    }

    /// Match all requests.
    pub fn match_all(&self, url: Option<&str>) -> Vec<&CacheEntry> {
        match url {
            Some(u) => self.entries.values().filter(|e| e.url == u).collect(),
            None => self.entries.values().collect(),
        }
    }

    /// Add entry.
    pub fn put(&mut self, url: &str, entry: CacheEntry) {
        self.entries.insert(url.to_string(), entry);
    }

    /// Add URL (simulated fetch and cache).
    pub fn add(&mut self, url: &str) -> Result<(), ServiceWorkerError> {
        // In real impl, would fetch the URL
        let entry = CacheEntry {
            url: url.to_string(),
            method: "GET".to_string(),
            status: 200,
            headers: HashMap::new(),
            body: Vec::new(),
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };
        self.put(url, entry);
        Ok(())
    }

    /// Add all URLs.
    pub fn add_all(&mut self, urls: &[&str]) -> Result<(), ServiceWorkerError> {
        for url in urls {
            self.add(url)?;
        }
        Ok(())
    }

    /// Delete entry.
    pub fn delete(&mut self, url: &str) -> bool {
        self.entries.remove(url).is_some()
    }

    /// Get all keys (URLs).
    pub fn keys(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }
}

// ==================== Cache Storage ====================

/// Cache storage (caches global).
#[derive(Debug, Default)]
pub struct CacheStorage {
    caches: HashMap<String, Cache>,
}

impl CacheStorage {
    /// Create new cache storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a cache (creates if doesn't exist).
    pub fn open(&mut self, name: &str) -> &mut Cache {
        self.caches.entry(name.to_string()).or_insert_with(|| Cache::new(name))
    }

    /// Check if cache exists.
    pub fn has(&self, name: &str) -> bool {
        self.caches.contains_key(name)
    }

    /// Delete a cache.
    pub fn delete(&mut self, name: &str) -> bool {
        self.caches.remove(name).is_some()
    }

    /// Get all cache names.
    pub fn keys(&self) -> Vec<&str> {
        self.caches.keys().map(|s| s.as_str()).collect()
    }

    /// Match across all caches.
    pub fn match_request(&self, url: &str) -> Option<&CacheEntry> {
        for cache in self.caches.values() {
            if let Some(entry) = cache.match_request(url) {
                return Some(entry);
            }
        }
        None
    }
}

// ==================== Client ====================

/// A client (controlled page).
#[derive(Debug, Clone)]
pub struct Client {
    /// Client ID.
    pub id: String,
    
    /// Client URL.
    pub url: Url,
    
    /// Client type.
    pub client_type: ClientType,
    
    /// Frame type.
    pub frame_type: FrameType,
    
    /// Visibility state.
    pub visibility_state: VisibilityState,
    
    /// Whether focused.
    pub focused: bool,
}

/// Client type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientType {
    Window,
    Worker,
    SharedWorker,
    All,
}

/// Frame type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Auxiliary,
    TopLevel,
    Nested,
    None,
}

/// Visibility state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityState {
    Hidden,
    Visible,
}

impl Client {
    /// Post message to client.
    ///
    /// Status: Not implemented - requires message channel between worker and client contexts.
    pub fn post_message(&self, _message: &str) -> Result<(), ServiceWorkerError> {
        Err(ServiceWorkerError::NotImplemented(
            "Client message passing requires cross-context communication".to_string(),
        ))
    }

    /// Focus the client.
    pub fn focus(&mut self) -> Result<(), ServiceWorkerError> {
        if self.client_type != ClientType::Window {
            return Err(ServiceWorkerError::StateError(
                "Can only focus window clients".to_string(),
            ));
        }
        self.focused = true;
        Ok(())
    }

    /// Navigate client to URL.
    ///
    /// Status: Not implemented - requires integration with navigation controller.
    pub fn navigate(&self, _url: &str) -> Result<(), ServiceWorkerError> {
        if self.client_type != ClientType::Window {
            return Err(ServiceWorkerError::StateError(
                "Can only navigate window clients".to_string(),
            ));
        }
        Err(ServiceWorkerError::NotImplemented(
            "Client navigation requires integration with browser navigation controller".to_string(),
        ))
    }
}

// ==================== Clients ====================

/// Clients API.
#[derive(Debug, Default)]
pub struct Clients {
    clients: HashMap<String, Client>,
}

impl Clients {
    /// Create new clients manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a client by ID.
    pub fn get(&self, id: &str) -> Option<&Client> {
        self.clients.get(id)
    }

    /// Match all clients.
    ///
    /// Note: Currently does not filter by controller status. All clients are treated as
    /// controlled when `include_uncontrolled` is false. Full implementation requires
    /// tracking which service worker controls each client.
    pub fn match_all(&self, options: ClientMatchOptions) -> Vec<&Client> {
        self.clients
            .values()
            .filter(|c| {
                // Note: Controller check not implemented - treating all as controlled
                // Full implementation would check: c.controller_id == Some(self.worker_id)
                if !options.include_uncontrolled {
                    // For now, assume all clients are controlled
                    true
                } else {
                    true
                };

                match options.client_type {
                    ClientType::All => true,
                    t => c.client_type == t,
                }
            })
            .collect()
    }

    /// Open a window.
    pub fn open_window(&mut self, url: &str) -> Result<Client, ServiceWorkerError> {
        let url = Url::parse(url).map_err(|e| ServiceWorkerError::NetworkError(e.to_string()))?;
        
        let id = format!("client-{}", uuid_simple());
        let client = Client {
            id: id.clone(),
            url,
            client_type: ClientType::Window,
            frame_type: FrameType::TopLevel,
            visibility_state: VisibilityState::Visible,
            focused: true,
        };
        
        self.clients.insert(id, client.clone());
        Ok(client)
    }

    /// Claim all clients.
    pub fn claim(&mut self) -> Result<(), ServiceWorkerError> {
        // Mark all matching clients as controlled
        // In real impl, would update each client's controller
        Ok(())
    }

    /// Add a client.
    pub fn add(&mut self, client: Client) {
        self.clients.insert(client.id.clone(), client);
    }

    /// Remove a client.
    pub fn remove(&mut self, id: &str) -> Option<Client> {
        self.clients.remove(id)
    }
}

/// Options for clients.matchAll().
#[derive(Debug, Clone, Default)]
pub struct ClientMatchOptions {
    pub include_uncontrolled: bool,
    pub client_type: ClientType,
}

impl Default for ClientType {
    fn default() -> Self {
        Self::Window
    }
}

// ==================== Fetch Event ====================

/// A fetch event.
#[derive(Debug, Clone)]
pub struct FetchEvent {
    /// Request URL.
    pub url: Url,
    
    /// Request method.
    pub method: String,
    
    /// Request headers.
    pub headers: HashMap<String, String>,
    
    /// Client ID.
    pub client_id: Option<String>,
    
    /// Is navigation request.
    pub is_navigation: bool,
    
    /// Is reload.
    pub is_reload: bool,
}

/// Fetch event response.
#[derive(Debug, Clone)]
pub struct FetchResponse {
    /// Status code.
    pub status: u16,
    
    /// Status text.
    pub status_text: String,
    
    /// Response headers.
    pub headers: HashMap<String, String>,
    
    /// Response body.
    pub body: Vec<u8>,
    
    /// Whether from cache.
    pub from_cache: bool,
}

impl FetchResponse {
    /// Create a network error response.
    pub fn network_error() -> Self {
        Self {
            status: 0,
            status_text: "Network Error".to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
            from_cache: false,
        }
    }

    /// Create a response from cache entry.
    pub fn from_cache(entry: &CacheEntry) -> Self {
        Self {
            status: entry.status,
            status_text: "OK".to_string(),
            headers: entry.headers.clone(),
            body: entry.body.clone(),
            from_cache: true,
        }
    }
}

// ==================== Service Worker Container ====================

/// Service worker container (navigator.serviceWorker).
pub struct ServiceWorkerContainer {
    /// Registrations by scope.
    registrations: Arc<RwLock<HashMap<String, ServiceWorkerRegistration>>>,
    
    /// Cache storage.
    pub caches: Arc<RwLock<CacheStorage>>,
    
    /// Clients.
    pub clients: Arc<RwLock<Clients>>,
    
    /// Event sender for state changes.
    event_tx: mpsc::UnboundedSender<ServiceWorkerEvent>,
}

/// Service worker events.
#[derive(Debug, Clone)]
pub enum ServiceWorkerEvent {
    /// State changed.
    StateChange {
        registration_scope: String,
        worker_id: ServiceWorkerId,
        new_state: ServiceWorkerState,
    },
    /// Update found.
    UpdateFound { registration_scope: String },
    /// Controller changed.
    ControllerChange { client_id: String },
    /// Message received.
    Message { data: String },
}

impl ServiceWorkerContainer {
    /// Create a new container.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<ServiceWorkerEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        
        (Self {
            registrations: Arc::new(RwLock::new(HashMap::new())),
            caches: Arc::new(RwLock::new(CacheStorage::new())),
            clients: Arc::new(RwLock::new(Clients::new())),
            event_tx,
        }, event_rx)
    }

    /// Register a service worker.
    pub async fn register(
        &self,
        script_url: &str,
        options: RegistrationOptions,
    ) -> Result<(), ServiceWorkerError> {
        let script_url = Url::parse(script_url)
            .map_err(|e| ServiceWorkerError::RegistrationFailed(e.to_string()))?;
        
        // Determine scope
        let scope = match options.scope {
            Some(s) => Url::parse(&s)
                .map_err(|e| ServiceWorkerError::RegistrationFailed(e.to_string()))?,
            None => {
                let mut scope = script_url.clone();
                scope.set_path(
                    script_url.path()
                        .rsplit_once('/')
                        .map(|(p, _)| p)
                        .unwrap_or("/")
                );
                scope
            }
        };
        
        let scope_str = scope.to_string();
        
        // Create or update registration
        let mut registrations = self.registrations.write().await;
        let registration = registrations
            .entry(scope_str.clone())
            .or_insert_with(|| ServiceWorkerRegistration::new(scope, options.update_via_cache));
        
        registration.update(script_url, options.worker_type);
        
        // Simulate install
        if let Some(ref mut worker) = registration.installing {
            worker.set_state(ServiceWorkerState::Installing);
        }
        registration.install_complete();
        
        let _ = self.event_tx.send(ServiceWorkerEvent::UpdateFound {
            registration_scope: scope_str,
        });
        
        Ok(())
    }

    /// Get registration for a URL.
    pub async fn get_registration(&self, url: &str) -> Option<String> {
        let url = Url::parse(url).ok()?;
        let registrations = self.registrations.read().await;
        
        // Find matching scope
        for (scope, _) in registrations.iter() {
            if url.as_str().starts_with(scope) {
                return Some(scope.clone());
            }
        }
        None
    }

    /// Get all registrations.
    pub async fn get_registrations(&self) -> Vec<String> {
        self.registrations.read().await.keys().cloned().collect()
    }

    /// Handle fetch event.
    pub async fn handle_fetch(&self, event: FetchEvent) -> Option<FetchResponse> {
        let url = event.url.to_string();
        
        // Check cache first
        let caches = self.caches.read().await;
        if let Some(entry) = caches.match_request(&url) {
            return Some(FetchResponse::from_cache(entry));
        }
        
        // No cache hit, return None to indicate network fetch needed
        None
    }

    /// Activate a waiting worker.
    pub async fn activate(&self, scope: &str) -> Result<(), ServiceWorkerError> {
        let mut registrations = self.registrations.write().await;
        let registration = registrations
            .get_mut(scope)
            .ok_or_else(|| ServiceWorkerError::NotFound(scope.to_string()))?;
        
        registration.activate();
        
        if let Some(ref worker) = registration.active {
            let _ = self.event_tx.send(ServiceWorkerEvent::StateChange {
                registration_scope: scope.to_string(),
                worker_id: worker.id,
                new_state: ServiceWorkerState::Activated,
            });
        }
        
        Ok(())
    }

    /// Unregister a service worker.
    pub async fn unregister(&self, scope: &str) -> Result<bool, ServiceWorkerError> {
        let mut registrations = self.registrations.write().await;
        if let Some(mut registration) = registrations.remove(scope) {
            registration.unregister();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl Default for ServiceWorkerContainer {
    fn default() -> Self {
        Self::new().0
    }
}

// ==================== Helpers ====================

/// Generate a simple UUID-like string.
fn uuid_simple() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!(
        "{:016x}-{:04x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_worker_creation() {
        let url = Url::parse("https://example.com/sw.js").unwrap();
        let worker = ServiceWorker::new(url.clone(), WorkerType::Classic);
        
        assert_eq!(worker.script_url, url);
        assert_eq!(worker.state, ServiceWorkerState::Parsed);
        assert!(!worker.is_active());
    }

    #[test]
    fn test_service_worker_state_transitions() {
        let url = Url::parse("https://example.com/sw.js").unwrap();
        let mut worker = ServiceWorker::new(url, WorkerType::Classic);
        
        worker.set_state(ServiceWorkerState::Installing);
        assert_eq!(worker.state, ServiceWorkerState::Installing);
        
        worker.set_state(ServiceWorkerState::Activated);
        assert!(worker.is_active());
    }

    #[test]
    fn test_registration() {
        let scope = Url::parse("https://example.com/").unwrap();
        let mut registration = ServiceWorkerRegistration::new(scope, UpdateViaCache::Imports);
        
        assert!(registration.active.is_none());
        
        let script = Url::parse("https://example.com/sw.js").unwrap();
        registration.update(script, WorkerType::Classic);
        
        assert!(registration.installing.is_some());
    }

    #[test]
    fn test_registration_lifecycle() {
        let scope = Url::parse("https://example.com/").unwrap();
        let mut registration = ServiceWorkerRegistration::new(scope, UpdateViaCache::Imports);
        
        let script = Url::parse("https://example.com/sw.js").unwrap();
        registration.update(script, WorkerType::Classic);
        
        // Installing -> Installed
        registration.install_complete();
        assert!(registration.waiting.is_some());
        assert!(registration.installing.is_none());
        
        // Installed -> Active
        registration.activate();
        assert!(registration.active.is_some());
        assert!(registration.waiting.is_none());
    }

    #[test]
    fn test_cache() {
        let mut cache = Cache::new("v1");
        
        cache.add("https://example.com/style.css").unwrap();
        
        assert!(cache.match_request("https://example.com/style.css").is_some());
        assert!(cache.match_request("https://example.com/other.css").is_none());
    }

    #[test]
    fn test_cache_delete() {
        let mut cache = Cache::new("v1");
        
        cache.add("https://example.com/style.css").unwrap();
        assert!(cache.delete("https://example.com/style.css"));
        assert!(cache.match_request("https://example.com/style.css").is_none());
    }

    #[test]
    fn test_cache_storage() {
        let mut storage = CacheStorage::new();
        
        assert!(!storage.has("v1"));
        
        storage.open("v1");
        assert!(storage.has("v1"));
        
        assert!(storage.delete("v1"));
        assert!(!storage.has("v1"));
    }

    #[test]
    fn test_clients() {
        let mut clients = Clients::new();
        
        let client = clients.open_window("https://example.com/").unwrap();
        assert_eq!(client.client_type, ClientType::Window);
        assert!(client.focused);
        
        assert!(clients.get(&client.id).is_some());
    }

    #[test]
    fn test_fetch_response_from_cache() {
        let entry = CacheEntry {
            url: "https://example.com/data.json".to_string(),
            method: "GET".to_string(),
            status: 200,
            headers: HashMap::new(),
            body: b"{}".to_vec(),
            cached_at: 0,
        };
        
        let response = FetchResponse::from_cache(&entry);
        assert_eq!(response.status, 200);
        assert!(response.from_cache);
    }

    #[tokio::test]
    async fn test_container_register() {
        let (container, _rx) = ServiceWorkerContainer::new();
        
        let result = container.register(
            "https://example.com/sw.js",
            RegistrationOptions::default(),
        ).await;
        
        assert!(result.is_ok());
        
        let registrations = container.get_registrations().await;
        assert_eq!(registrations.len(), 1);
    }

    #[tokio::test]
    async fn test_container_unregister() {
        let (container, _rx) = ServiceWorkerContainer::new();
        
        container.register(
            "https://example.com/sw.js",
            RegistrationOptions::default(),
        ).await.unwrap();
        
        let scope = container.get_registrations().await[0].clone();
        let result = container.unregister(&scope).await;
        
        assert!(result.is_ok());
        assert!(result.unwrap());
        
        let registrations = container.get_registrations().await;
        assert!(registrations.is_empty());
    }

    #[test]
    fn test_cache_keys() {
        let mut cache = Cache::new("test");
        cache.add("https://example.com/a.js").unwrap();
        cache.add("https://example.com/b.js").unwrap();
        
        let keys = cache.keys();
        assert_eq!(keys.len(), 2);
    }
}


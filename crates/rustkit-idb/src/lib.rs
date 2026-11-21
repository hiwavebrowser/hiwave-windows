//! # RustKit IndexedDB
//!
//! IndexedDB implementation for the RustKit browser engine.
//!
//! ## Features
//!
//! - **IDBFactory**: `indexedDB.open()`, `deleteDatabase()`
//! - **IDBDatabase**: Object store management
//! - **IDBObjectStore**: CRUD operations
//! - **IDBTransaction**: readonly, readwrite, versionchange
//! - **IDBIndex**: Secondary indexes
//! - **IDBCursor**: Iteration over records
//!
//! ## Architecture
//!
//! ```text
//! IDBFactory (window.indexedDB)
//!     │
//!     └── IDBDatabase
//!             │
//!             ├── IDBObjectStore
//!             │       ├── IDBIndex
//!             │       └── Records
//!             │
//!             └── IDBTransaction
//!                     └── IDBRequest
//! ```

use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};

// ==================== Errors ====================

/// IndexedDB errors.
#[derive(Error, Debug, Clone)]
pub enum IDBError {
    #[error("Database not found: {0}")]
    NotFoundError(String),

    #[error("Constraint error: {0}")]
    ConstraintError(String),

    #[error("Data error: {0}")]
    DataError(String),

    #[error("Invalid state: {0}")]
    InvalidStateError(String),

    #[error("Transaction inactive")]
    TransactionInactiveError,

    #[error("Read only")]
    ReadOnlyError,

    #[error("Version error: {0}")]
    VersionError(String),

    #[error("Abort error: {0}")]
    AbortError(String),

    #[error("Unknown error: {0}")]
    UnknownError(String),
}

// ==================== Types ====================

/// Unique request ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

impl RequestId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Key path for object stores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyPath {
    /// No key path (out-of-line keys).
    None,
    /// Single property.
    Single(String),
    /// Multiple properties (compound key).
    Multiple(Vec<String>),
}

impl KeyPath {
    /// Extract key from value.
    pub fn extract(&self, value: &JsonValue) -> Option<JsonValue> {
        match self {
            KeyPath::None => None,
            KeyPath::Single(path) => value.get(path).cloned(),
            KeyPath::Multiple(paths) => {
                let keys: Vec<JsonValue> = paths
                    .iter()
                    .filter_map(|p| value.get(p).cloned())
                    .collect();
                if keys.len() == paths.len() {
                    Some(JsonValue::Array(keys))
                } else {
                    None
                }
            }
        }
    }
}

/// A stored record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub key: JsonValue,
    pub value: JsonValue,
}

// ==================== IDBIndex ====================

/// An index on an object store.
#[derive(Debug, Clone)]
pub struct IDBIndex {
    /// Index name.
    pub name: String,
    
    /// Key path.
    pub key_path: KeyPath,
    
    /// Whether keys must be unique.
    pub unique: bool,
    
    /// Multi-entry (for array values).
    pub multi_entry: bool,
    
    /// Entries (index key -> primary keys).
    entries: HashMap<String, Vec<String>>,
}

impl IDBIndex {
    /// Create a new index.
    pub fn new(name: &str, key_path: KeyPath, unique: bool, multi_entry: bool) -> Self {
        Self {
            name: name.to_string(),
            key_path,
            unique,
            multi_entry,
            entries: HashMap::new(),
        }
    }

    /// Add entry.
    pub fn add_entry(&mut self, index_key: &str, primary_key: &str) -> Result<(), IDBError> {
        if self.unique && self.entries.contains_key(index_key) {
            return Err(IDBError::ConstraintError(format!(
                "Duplicate key in unique index: {}",
                index_key
            )));
        }
        
        self.entries
            .entry(index_key.to_string())
            .or_default()
            .push(primary_key.to_string());
        Ok(())
    }

    /// Get primary keys for index key.
    pub fn get(&self, index_key: &str) -> Option<&Vec<String>> {
        self.entries.get(index_key)
    }

    /// Delete entry.
    pub fn delete_entry(&mut self, index_key: &str, primary_key: &str) {
        if let Some(keys) = self.entries.get_mut(index_key) {
            keys.retain(|k| k != primary_key);
            if keys.is_empty() {
                self.entries.remove(index_key);
            }
        }
    }
}

// ==================== IDBObjectStore ====================

/// An object store.
#[derive(Debug)]
pub struct IDBObjectStore {
    /// Store name.
    pub name: String,
    
    /// Key path.
    pub key_path: KeyPath,
    
    /// Auto-increment.
    pub auto_increment: bool,
    
    /// Records.
    records: HashMap<String, Record>,
    
    /// Indexes.
    indexes: HashMap<String, IDBIndex>,
    
    /// Next auto-increment key.
    next_key: u64,
}

impl IDBObjectStore {
    /// Create a new object store.
    pub fn new(name: &str, key_path: KeyPath, auto_increment: bool) -> Self {
        Self {
            name: name.to_string(),
            key_path,
            auto_increment,
            records: HashMap::new(),
            indexes: HashMap::new(),
            next_key: 1,
        }
    }

    /// Get a record.
    pub fn get(&self, key: &str) -> Option<&Record> {
        self.records.get(key)
    }

    /// Get all records.
    pub fn get_all(&self, count: Option<usize>) -> Vec<&Record> {
        let mut records: Vec<_> = self.records.values().collect();
        if let Some(n) = count {
            records.truncate(n);
        }
        records
    }

    /// Get all keys.
    pub fn get_all_keys(&self, count: Option<usize>) -> Vec<&str> {
        let mut keys: Vec<_> = self.records.keys().map(|s| s.as_str()).collect();
        if let Some(n) = count {
            keys.truncate(n);
        }
        keys
    }

    /// Add a record (fails if key exists).
    pub fn add(&mut self, value: JsonValue, key: Option<JsonValue>) -> Result<String, IDBError> {
        let key = self.resolve_key(&value, key)?;
        let key_str = json_to_key_string(&key);
        
        if self.records.contains_key(&key_str) {
            return Err(IDBError::ConstraintError(format!(
                "Key already exists: {}",
                key_str
            )));
        }
        
        self.insert_record(key_str.clone(), key, value)?;
        Ok(key_str)
    }

    /// Put a record (overwrites if exists).
    pub fn put(&mut self, value: JsonValue, key: Option<JsonValue>) -> Result<String, IDBError> {
        let key = self.resolve_key(&value, key)?;
        let key_str = json_to_key_string(&key);
        
        // Remove old index entries if updating
        if self.records.contains_key(&key_str) {
            self.remove_from_indexes(&key_str);
        }
        
        self.insert_record(key_str.clone(), key, value)?;
        Ok(key_str)
    }

    /// Delete a record.
    pub fn delete(&mut self, key: &str) -> bool {
        if self.records.remove(key).is_some() {
            self.remove_from_indexes(key);
            true
        } else {
            false
        }
    }

    /// Clear all records.
    pub fn clear(&mut self) {
        self.records.clear();
        for index in self.indexes.values_mut() {
            index.entries.clear();
        }
    }

    /// Count records.
    pub fn count(&self) -> usize {
        self.records.len()
    }

    /// Create an index.
    pub fn create_index(
        &mut self,
        name: &str,
        key_path: KeyPath,
        unique: bool,
        multi_entry: bool,
    ) -> Result<(), IDBError> {
        if self.indexes.contains_key(name) {
            return Err(IDBError::ConstraintError(format!(
                "Index already exists: {}",
                name
            )));
        }
        
        let mut index = IDBIndex::new(name, key_path.clone(), unique, multi_entry);
        
        // Build index from existing records
        for (primary_key, record) in &self.records {
            if let Some(index_key) = key_path.extract(&record.value) {
                index.add_entry(&json_to_key_string(&index_key), primary_key)?;
            }
        }
        
        self.indexes.insert(name.to_string(), index);
        Ok(())
    }

    /// Delete an index.
    pub fn delete_index(&mut self, name: &str) -> bool {
        self.indexes.remove(name).is_some()
    }

    /// Get an index.
    pub fn index(&self, name: &str) -> Option<&IDBIndex> {
        self.indexes.get(name)
    }

    /// Get index names.
    pub fn index_names(&self) -> Vec<&str> {
        self.indexes.keys().map(|s| s.as_str()).collect()
    }

    /// Resolve key from value or explicit key.
    fn resolve_key(&mut self, value: &JsonValue, key: Option<JsonValue>) -> Result<JsonValue, IDBError> {
        match (&self.key_path, key, self.auto_increment) {
            // Explicit key provided
            (_, Some(k), _) => Ok(k),
            
            // Key path extracts key
            (KeyPath::Single(_) | KeyPath::Multiple(_), None, _) => {
                self.key_path.extract(value).ok_or_else(|| {
                    IDBError::DataError("Could not extract key from value".to_string())
                })
            }
            
            // Auto-increment
            (KeyPath::None, None, true) => {
                let key = self.next_key;
                self.next_key += 1;
                Ok(JsonValue::Number(key.into()))
            }
            
            // No key and no auto-increment
            (KeyPath::None, None, false) => {
                Err(IDBError::DataError("No key provided and no auto-increment".to_string()))
            }
        }
    }

    /// Insert record and update indexes.
    fn insert_record(&mut self, key_str: String, key: JsonValue, value: JsonValue) -> Result<(), IDBError> {
        // Update indexes
        for (_, index) in &mut self.indexes {
            if let Some(index_key) = index.key_path.extract(&value) {
                index.add_entry(&json_to_key_string(&index_key), &key_str)?;
            }
        }
        
        self.records.insert(key_str, Record { key, value });
        Ok(())
    }

    /// Remove from all indexes.
    fn remove_from_indexes(&mut self, primary_key: &str) {
        if let Some(record) = self.records.get(primary_key) {
            for (_, index) in &mut self.indexes {
                if let Some(index_key) = index.key_path.extract(&record.value) {
                    index.delete_entry(&json_to_key_string(&index_key), primary_key);
                }
            }
        }
    }
}

// ==================== IDBTransaction ====================

/// Transaction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionMode {
    ReadOnly,
    ReadWrite,
    VersionChange,
}

/// Transaction state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    Active,
    Committing,
    Finished,
}

/// A database transaction.
#[derive(Debug)]
pub struct IDBTransaction {
    /// Transaction mode.
    pub mode: TransactionMode,
    
    /// Object store names in scope.
    pub scope: Vec<String>,
    
    /// State.
    pub state: TransactionState,
    
    /// Error if aborted.
    pub error: Option<IDBError>,
}

impl IDBTransaction {
    /// Create a new transaction.
    pub fn new(mode: TransactionMode, scope: Vec<String>) -> Self {
        Self {
            mode,
            scope,
            state: TransactionState::Active,
            error: None,
        }
    }

    /// Check if active.
    pub fn is_active(&self) -> bool {
        self.state == TransactionState::Active
    }

    /// Check if store is in scope.
    pub fn has_store(&self, name: &str) -> bool {
        self.scope.iter().any(|s| s == name)
    }

    /// Abort the transaction.
    pub fn abort(&mut self, error: IDBError) {
        self.state = TransactionState::Finished;
        self.error = Some(error);
    }

    /// Commit the transaction.
    pub fn commit(&mut self) {
        if self.state == TransactionState::Active {
            self.state = TransactionState::Committing;
            // In real impl, would flush changes
            self.state = TransactionState::Finished;
        }
    }
}

// ==================== IDBDatabase ====================

/// A database.
#[derive(Debug)]
pub struct IDBDatabase {
    /// Database name.
    pub name: String,
    
    /// Version.
    pub version: u64,
    
    /// Object stores.
    stores: HashMap<String, IDBObjectStore>,
}

impl IDBDatabase {
    /// Create a new database.
    pub fn new(name: &str, version: u64) -> Self {
        Self {
            name: name.to_string(),
            version,
            stores: HashMap::new(),
        }
    }

    /// Get object store names.
    pub fn object_store_names(&self) -> Vec<&str> {
        self.stores.keys().map(|s| s.as_str()).collect()
    }

    /// Create object store (only in versionchange transaction).
    pub fn create_object_store(
        &mut self,
        name: &str,
        key_path: KeyPath,
        auto_increment: bool,
    ) -> Result<(), IDBError> {
        if self.stores.contains_key(name) {
            return Err(IDBError::ConstraintError(format!(
                "Object store already exists: {}",
                name
            )));
        }
        
        self.stores.insert(
            name.to_string(),
            IDBObjectStore::new(name, key_path, auto_increment),
        );
        Ok(())
    }

    /// Delete object store (only in versionchange transaction).
    pub fn delete_object_store(&mut self, name: &str) -> Result<(), IDBError> {
        self.stores.remove(name).ok_or_else(|| {
            IDBError::NotFoundError(format!("Object store not found: {}", name))
        })?;
        Ok(())
    }

    /// Get object store.
    pub fn object_store(&self, name: &str) -> Option<&IDBObjectStore> {
        self.stores.get(name)
    }

    /// Get object store mutably.
    pub fn object_store_mut(&mut self, name: &str) -> Option<&mut IDBObjectStore> {
        self.stores.get_mut(name)
    }

    /// Start a transaction.
    pub fn transaction(&self, stores: Vec<String>, mode: TransactionMode) -> Result<IDBTransaction, IDBError> {
        // Validate store names
        for name in &stores {
            if !self.stores.contains_key(name) {
                return Err(IDBError::NotFoundError(format!(
                    "Object store not found: {}",
                    name
                )));
            }
        }
        
        Ok(IDBTransaction::new(mode, stores))
    }
}

// ==================== IDBFactory ====================

/// IDBFactory events.
#[derive(Debug, Clone)]
pub enum IDBEvent {
    /// Upgrade needed.
    UpgradeNeeded {
        db_name: String,
        old_version: u64,
        new_version: u64,
    },
    /// Database opened.
    Success { db_name: String },
    /// Error occurred.
    Error { db_name: String, error: IDBError },
    /// Database blocked.
    Blocked { db_name: String },
}

/// IDBFactory (window.indexedDB).
pub struct IDBFactory {
    /// Databases.
    databases: Arc<RwLock<HashMap<String, IDBDatabase>>>,
    
    /// Event sender.
    event_tx: mpsc::UnboundedSender<IDBEvent>,
}

impl IDBFactory {
    /// Create a new factory.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<IDBEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        
        (Self {
            databases: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }, event_rx)
    }

    /// Open a database.
    pub async fn open(&self, name: &str, version: Option<u64>) -> Result<(), IDBError> {
        let mut databases = self.databases.write().await;
        
        let current_version = databases.get(name).map(|db| db.version).unwrap_or(0);
        let requested_version = version.unwrap_or(1);
        
        if requested_version < current_version {
            let error = IDBError::VersionError(format!(
                "Requested version {} is less than current version {}",
                requested_version, current_version
            ));
            let _ = self.event_tx.send(IDBEvent::Error {
                db_name: name.to_string(),
                error: error.clone(),
            });
            return Err(error);
        }
        
        let needs_upgrade = requested_version > current_version;
        
        if needs_upgrade {
            // Fire upgrade needed event
            let _ = self.event_tx.send(IDBEvent::UpgradeNeeded {
                db_name: name.to_string(),
                old_version: current_version,
                new_version: requested_version,
            });
        }
        
        // Create or update database
        databases.insert(name.to_string(), IDBDatabase::new(name, requested_version));
        
        let _ = self.event_tx.send(IDBEvent::Success {
            db_name: name.to_string(),
        });
        
        Ok(())
    }

    /// Delete a database.
    pub async fn delete_database(&self, name: &str) -> Result<(), IDBError> {
        let mut databases = self.databases.write().await;
        databases.remove(name);
        Ok(())
    }

    /// Get database names.
    pub async fn databases(&self) -> Vec<DatabaseInfo> {
        let databases = self.databases.read().await;
        databases
            .iter()
            .map(|(name, db)| DatabaseInfo {
                name: name.clone(),
                version: db.version,
            })
            .collect()
    }

    /// Get a database.
    pub async fn get(&self, name: &str) -> Option<IDBDatabase> {
        let databases = self.databases.read().await;
        databases.get(name).map(|db| IDBDatabase {
            name: db.name.clone(),
            version: db.version,
            stores: db.stores.iter().map(|(k, v)| {
                (k.clone(), IDBObjectStore {
                    name: v.name.clone(),
                    key_path: v.key_path.clone(),
                    auto_increment: v.auto_increment,
                    records: v.records.clone(),
                    indexes: v.indexes.clone(),
                    next_key: v.next_key,
                })
            }).collect(),
        })
    }

    /// Execute an operation on a database.
    pub async fn with_database<F, R>(&self, name: &str, f: F) -> Result<R, IDBError>
    where
        F: FnOnce(&mut IDBDatabase) -> Result<R, IDBError>,
    {
        let mut databases = self.databases.write().await;
        let db = databases.get_mut(name).ok_or_else(|| {
            IDBError::NotFoundError(format!("Database not found: {}", name))
        })?;
        f(db)
    }
}

impl Default for IDBFactory {
    fn default() -> Self {
        Self::new().0
    }
}

/// Database info for databases() API.
#[derive(Debug, Clone)]
pub struct DatabaseInfo {
    pub name: String,
    pub version: u64,
}

// ==================== IDBCursor ====================

/// Cursor direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorDirection {
    #[default]
    Next,
    NextUnique,
    Prev,
    PrevUnique,
}

/// A cursor for iterating records.
#[derive(Debug)]
pub struct IDBCursor {
    /// Direction.
    pub direction: CursorDirection,
    
    /// Source (store name or index name).
    pub source: String,
    
    /// Current key.
    pub key: Option<String>,
    
    /// Current primary key.
    pub primary_key: Option<String>,
    
    /// Keys to iterate (internal).
    keys: Vec<String>,
    
    /// Current position.
    position: usize,
    
    /// Done.
    pub done: bool,
}

impl IDBCursor {
    /// Create a new cursor.
    pub fn new(source: &str, keys: Vec<String>, direction: CursorDirection) -> Self {
        let mut sorted_keys = keys;
        match direction {
            CursorDirection::Prev | CursorDirection::PrevUnique => {
                sorted_keys.reverse();
            }
            _ => {}
        }
        
        Self {
            direction,
            source: source.to_string(),
            key: None,
            primary_key: None,
            keys: sorted_keys,
            position: 0,
            done: false,
        }
    }

    /// Advance to next record.
    pub fn continue_cursor(&mut self) -> bool {
        if self.position < self.keys.len() {
            let key = self.keys[self.position].clone();
            self.key = Some(key.clone());
            self.primary_key = Some(key);
            self.position += 1;
            true
        } else {
            self.done = true;
            self.key = None;
            self.primary_key = None;
            false
        }
    }

    /// Advance by count.
    pub fn advance(&mut self, count: usize) -> bool {
        for _ in 0..count {
            if !self.continue_cursor() {
                return false;
            }
        }
        true
    }
}

// ==================== Helpers ====================

/// Convert JSON value to key string.
fn json_to_key_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(json_to_key_string).collect();
            format!("[{}]", parts.join(","))
        }
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_path_single() {
        let path = KeyPath::Single("id".to_string());
        let value = serde_json::json!({"id": 42, "name": "test"});
        
        let key = path.extract(&value).unwrap();
        assert_eq!(key, serde_json::json!(42));
    }

    #[test]
    fn test_key_path_multiple() {
        let path = KeyPath::Multiple(vec!["a".to_string(), "b".to_string()]);
        let value = serde_json::json!({"a": 1, "b": 2});
        
        let key = path.extract(&value).unwrap();
        assert_eq!(key, serde_json::json!([1, 2]));
    }

    #[test]
    fn test_object_store_add() {
        let mut store = IDBObjectStore::new("test", KeyPath::Single("id".to_string()), false);
        
        let value = serde_json::json!({"id": "key1", "data": "hello"});
        let key = store.add(value, None).unwrap();
        
        assert_eq!(key, "key1");
        assert!(store.get("key1").is_some());
    }

    #[test]
    fn test_object_store_put() {
        let mut store = IDBObjectStore::new("test", KeyPath::Single("id".to_string()), false);
        
        let value1 = serde_json::json!({"id": "key1", "data": "v1"});
        store.add(value1, None).unwrap();
        
        let value2 = serde_json::json!({"id": "key1", "data": "v2"});
        store.put(value2, None).unwrap();
        
        let record = store.get("key1").unwrap();
        assert_eq!(record.value["data"], "v2");
    }

    #[test]
    fn test_object_store_auto_increment() {
        let mut store = IDBObjectStore::new("test", KeyPath::None, true);
        
        let key1 = store.add(serde_json::json!({"data": "a"}), None).unwrap();
        let key2 = store.add(serde_json::json!({"data": "b"}), None).unwrap();
        
        assert_eq!(key1, "1");
        assert_eq!(key2, "2");
    }

    #[test]
    fn test_object_store_delete() {
        let mut store = IDBObjectStore::new("test", KeyPath::None, true);
        
        let key = store.add(serde_json::json!({"data": "test"}), None).unwrap();
        assert!(store.get(&key).is_some());
        
        store.delete(&key);
        assert!(store.get(&key).is_none());
    }

    #[test]
    fn test_index() {
        let mut store = IDBObjectStore::new("test", KeyPath::Single("id".to_string()), false);
        store.create_index("by_name", KeyPath::Single("name".to_string()), false, false).unwrap();
        
        store.add(serde_json::json!({"id": "1", "name": "Alice"}), None).unwrap();
        store.add(serde_json::json!({"id": "2", "name": "Bob"}), None).unwrap();
        
        let index = store.index("by_name").unwrap();
        assert!(index.get("Alice").is_some());
    }

    #[test]
    fn test_database() {
        let mut db = IDBDatabase::new("test", 1);
        
        db.create_object_store("users", KeyPath::Single("id".to_string()), false).unwrap();
        
        assert!(db.object_store("users").is_some());
        assert_eq!(db.object_store_names().len(), 1);
    }

    #[test]
    fn test_transaction() {
        let db = IDBDatabase::new("test", 1);
        let tx = db.transaction(vec![], TransactionMode::ReadOnly).unwrap();
        
        assert!(tx.is_active());
        assert_eq!(tx.mode, TransactionMode::ReadOnly);
    }

    #[test]
    fn test_cursor() {
        let keys = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut cursor = IDBCursor::new("store", keys, CursorDirection::Next);
        
        assert!(cursor.continue_cursor());
        assert_eq!(cursor.key, Some("a".to_string()));
        
        assert!(cursor.continue_cursor());
        assert_eq!(cursor.key, Some("b".to_string()));
        
        assert!(cursor.continue_cursor());
        assert_eq!(cursor.key, Some("c".to_string()));
        
        assert!(!cursor.continue_cursor());
        assert!(cursor.done);
    }

    #[tokio::test]
    async fn test_factory_open() {
        let (factory, _rx) = IDBFactory::new();
        
        factory.open("test", Some(1)).await.unwrap();
        
        let dbs = factory.databases().await;
        assert_eq!(dbs.len(), 1);
        assert_eq!(dbs[0].name, "test");
        assert_eq!(dbs[0].version, 1);
    }

    #[tokio::test]
    async fn test_factory_delete() {
        let (factory, _rx) = IDBFactory::new();
        
        factory.open("test", Some(1)).await.unwrap();
        factory.delete_database("test").await.unwrap();
        
        let dbs = factory.databases().await;
        assert!(dbs.is_empty());
    }

    #[test]
    fn test_unique_index() {
        let mut store = IDBObjectStore::new("test", KeyPath::Single("id".to_string()), false);
        store.create_index("by_email", KeyPath::Single("email".to_string()), true, false).unwrap();
        
        store.add(serde_json::json!({"id": "1", "email": "a@test.com"}), None).unwrap();
        
        // Should fail - duplicate email
        let result = store.add(serde_json::json!({"id": "2", "email": "a@test.com"}), None);
        assert!(result.is_err());
    }
}


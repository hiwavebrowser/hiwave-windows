# RustKit Service Workers

This crate provides a partial implementation of the Service Worker API for the RustKit browser engine.

## Current Status

### ✅ Implemented Features

**Core Infrastructure:**
- Service worker lifecycle states (installing, installed, activating, activated, redundant)
- Service worker registration with scope management
- Registration updates and unregistration
- Multiple service worker containers
- Fetch event interception skeleton
- Cache API skeleton (Cache and CacheStorage types)
- Client management types
- Background sync API skeleton

**Lifecycle Management:**
- `install` event dispatch
- `activate` event dispatch
- State transitions
- Update checking

### ❌ Not Yet Implemented

**Message Passing:**
- `postMessage()` between service worker and clients
- `postMessage()` from client to service worker
- MessageChannel/MessagePort integration

**Reason:** Requires bidirectional communication channels between worker context (background JavaScript execution) and client contexts (main browsing contexts). This needs:
- Worker thread isolation
- Structured clone algorithm for message serialization
- Event loop integration across contexts

**Navigation Control:**
- `client.navigate()` - Navigate a client window from service worker
- `clients.openWindow()` - Open new browser window

**Reason:** Requires integration with browser navigation controller and window management system.

**Client Management:**
- Controller tracking (which service worker controls which client)
- `clients.claim()` - Claim uncontrolled clients
- Filtering clients by controller status

**Reason:** Requires tracking service worker → client relationships and coordinating with page lifecycle.

**Cache API Implementation:**
- Actual cache storage and retrieval
- Cache matching strategies
- Cache quota management

**Reason:** Full implementation requires IndexedDB integration for persistent storage.

**Background Sync:**
- Actual background task scheduling
- Retry logic
- System integration for background execution

**Reason:** Requires OS-level background task scheduling and battery/network monitoring.

## Architecture

The service worker implementation is designed in layers:

1. **Types Layer** - Data structures representing service workers, registrations, clients
2. **State Management** - Lifecycle state machines and transitions
3. **Event System** - Event dispatch skeleton
4. **Integration Points** (Not Implemented) - Connections to worker execution, IPC, storage

## Future Work

To complete service worker support, the following components need implementation:

1. **Worker Context Execution**
   - JavaScript execution in isolated worker threads
   - Worker global scope with service worker APIs
   - Import scripts support

2. **Message Infrastructure**
   - Structured clone algorithm
   - Message channels
   - Event dispatch across contexts

3. **Storage Integration**
   - IndexedDB backend for Cache API
   - Quota management
   - Persistence layer

4. **System Integration**
   - Background task scheduling
   - Network monitoring
   - Battery status integration

5. **Navigation Integration**
   - Hook into browser navigation controller
   - Window management APIs

## Usage Notes

This crate currently provides:
- Type definitions for building service worker-aware applications
- Skeleton infrastructure for testing
- Foundation for future full implementation

For production use, service worker features should be disabled or wrapped in `cfg(feature = "service-workers-full")` feature gates.

## Error Handling

Methods that are not yet implemented return `ServiceWorkerError::NotImplemented` with descriptive messages indicating what's required for full implementation.

## Testing

The crate includes unit tests for implemented functionality:
- Lifecycle state transitions
- Registration management
- Type definitions

To run tests:
```bash
cargo test -p rustkit-sw
```

## References

- [Service Worker Specification](https://w3c.github.io/ServiceWorker/)
- [MDN Service Worker API](https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API)

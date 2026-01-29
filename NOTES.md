# Browser Session Orchestrator

## 1. Summary

This project implements a **browser session orchestrator** in Rust using **Restate** for durable, stateful execution and **Axum** as a thin HTTP compatibility layer.
The tester has been written in go encompassing TTL timeout checks, session/worker scaling, health/status checks, and notable edge cases.

Restate is responsible for:
- Persisting the **worker/session pool state** across failures
- Performing **durable side effects** (starting and stopping `steel-browser` processes) via `ctx.run`
- Coordinating retries and recovery during worker startup and health checks

Each `steel-browser` instance runs exactly one browser session and exposes its own HTTP API.  
The orchestrator:
- Spawns workers on dynamically allocated ports
- Maintains a mapping between **sessions and workers**
- Routes session-scoped requests to the correct worker based on persisted state

Axum serves only as a **schema adapter**, exposing endpoints compatible with the original `steel-browser` API and forwarding requests to Restate. All orchestration logic and state management live exclusively in Restate.

The primary design principle is **statefulness first**: session and worker state is always derived from Restate-managed state, never from in-memory caches.

---

## 2. Production Gaps

Several areas would break or degrade under real production load:

- **Process lifecycle management**
  - Spawned `steel-browser` processes are not supervised or reaped
  - Worker crashes after startup are not detected or cleaned up
- **Resource exhaustion**
  - Port allocation is TOCTOU-prone and not concurrency-safe
  - No CPU, memory, or file descriptor limits per worker
- **Backpressure and admission control**
  - Session creation is unbounded
  - No rate limiting or queuing during spikes
- **Observability**
  - No structured logging, metrics, or tracing
  - Worker stdout/stderr is not collected
- **Security**
  - No authentication between Axum, Restate, and workers
  - No isolation beyond OS process boundaries

While the system can handle ~1000 sessions in controlled conditions, these gaps would surface quickly under sustained or adversarial load.

---

## 3. Improvements

With more time, the following improvements would be prioritized:

- Replace ad-hoc port scanning with a **robust port allocation strategy**
- Add **explicit worker supervision** (restart, timeout, cleanup on failure)
- Introduce **typed state models** instead of raw JSON blobs in Restate
- Return **structured responses** instead of raw strings
- Add metrics for:
  - active sessions
  - worker startup latency
  - retry counts and failure rates
- Formalize idempotency guarantees for session creation and deletion

Longer-term, the worker abstraction would be generalized so `steel-browser` is just one backend among others.

---

## 4. Scaling

The current scaling model assumes **one orchestrator per region**, with each orchestrator managing workers local to that region. This minimizes cross-region latency and isolates failure domains.

In a Kubernetes-based deployment:
- A **single logical orchestrator** could be used
- Workers would be scheduled as pods rather than OS processes
- Session routing would rely on Kubernetes networking instead of local ports

This introduces additional complexity:
- ingress and service routing
- cross-node latency
- aligning pod lifecycle with session lifecycle

The current design intentionally avoids these complexities to first validate Restate’s durability and execution model.

---

## 5. Trade-offs

This project optimizes for:
- **Durability**
- **Explicit, persisted state**
- **Correctness under retries and partial failures**

It intentionally sacrifices:
- Simplicity of deployment
- Stateless request handling
- HTTP-layer ergonomics

Given the one-week time constraint, the focus was on **Restate’s execution and persistence model** with orchestrating parallel reliable sessions, rather than building a fully production-ready control plane. The project will continue to evolve, particularly around Kubernetes integration.


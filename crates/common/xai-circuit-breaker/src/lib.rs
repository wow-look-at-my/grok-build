#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_precision_loss)] // Hits predate the gate

//! Shared circuit breaker.

mod breaker;
mod clock;
mod config;
#[cfg(feature = "grpc")]
mod grpc;
mod observer;
mod registry;
mod retry_policy;
mod state;
mod window;

pub use breaker::CircuitBreaker;
#[cfg(any(test, feature = "test-hooks"))]
pub use clock::MockClock;
pub use clock::{Clock, SystemClock};
pub use config::{BreakerConfig, default_failure_codes, parse_failure_codes};
#[cfg(feature = "grpc")]
pub use grpc::GrpcRetryPolicy;
pub use observer::{NoopObserver, Observer};
pub use registry::CircuitBreakerRegistry;
pub use retry_policy::{Disposition, RetryPolicy};
pub use state::{BreakerOpen, BreakerState, Outcome};


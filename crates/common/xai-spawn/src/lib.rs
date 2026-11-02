//! Spawn wrappers that box the future or closure before tokio sees it.
#![allow(clippy::disallowed_methods)]

use std::future::Future;
use std::pin::Pin;

use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

type SendFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type LocalFuture<T> = Pin<Box<dyn Future<Output = T>>>;
type BlockingFn<R> = Box<dyn FnOnce() -> R + Send>;

/// [`tokio::spawn`] with the future type erased.
#[track_caller]
pub fn spawn<F>(f: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(Box::pin(f) as SendFuture<F::Output>)
}

/// [`Handle::spawn`] with the future type erased.
#[track_caller]
pub fn spawn_on<F>(handle: &Handle, f: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    handle.spawn(Box::pin(f) as SendFuture<F::Output>)
}

/// [`JoinSet::spawn`] with the future type erased.
#[track_caller]
pub fn spawn_in<F>(set: &mut JoinSet<F::Output>, f: F) -> AbortHandle
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    set.spawn(Box::pin(f) as SendFuture<F::Output>)
}

/// [`tokio::task::spawn_local`] with the future type erased.
#[track_caller]
pub fn spawn_local<F>(f: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    tokio::task::spawn_local(Box::pin(f) as LocalFuture<F::Output>)
}

/// [`tokio::task::spawn_blocking`] with the closure type erased.
#[track_caller]
pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(Box::new(f) as BlockingFn<R>)
}

/// [`Handle::spawn_blocking`] with the closure type erased.
#[track_caller]
pub fn spawn_blocking_on<F, R>(handle: &Handle, f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    handle.spawn_blocking(Box::new(f) as BlockingFn<R>)
}

#[cfg(test)]
mod tests;

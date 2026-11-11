//! The OS hosts of psh: the real processes, pipes and files behind the engine's `Host`
//! trait (ARCH.md "psh-os", research/execution-core.md §5).
//!
//! - [`unix`]: macOS, Linux and Android.
//! - [`windows`]: Windows (x64 and arm64).
//!
//! The platform-neutral parts ([`lifecycle`], the timer thread, [`Leftovers`]) live
//! outside the platform modules and are shared by both hosts.

pub mod lifecycle;
mod timer;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

pub use timer::sleep;

/// The fate of a run's leftovers when its host is dropped (D1067).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leftovers {
    /// TERM to every group of the run, KILL after the grace period. Only detached
    /// (`nohup`, `setsid`) and disowned externals survive. The default.
    Kill,
    /// Leave them running, as bash does.
    Keep,
}

/// Starts a helper thread with every asynchronous signal blocked from its first
/// instruction (Unix): signals for the process then reach the embedder's threads or,
/// standalone, the engine thread, never ours (a `kill $$` acts before `kill` returns).
pub(crate) fn spawn_helper<F: FnOnce() + Send + 'static>(
    name: &str,
    stack: usize,
    f: F,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    #[cfg(unix)]
    let old = unix::signals::block_all_in_thread();
    let r = std::thread::Builder::new()
        .name(name.into())
        .stack_size(stack)
        .spawn(f);
    #[cfg(unix)]
    unix::signals::restore_mask(old);
    r
}

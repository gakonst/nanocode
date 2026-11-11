//! Resource limits (`ulimit`): the process's, read with `getrlimit`, and the child's,
//! set with `setrlimit` in the fork path of spawn (posix_spawn cannot). The process's
//! own limits are never changed (D1293).

use psh_engine::host::{Limit, Rlimit};

/// The type `getrlimit`/`setrlimit` take for a resource on this platform.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) type Resource = libc::__rlimit_resource_t;
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(crate) type Resource = libc::c_int;

/// The platform's resource for `r`, if it has one.
pub(crate) fn resource(r: Rlimit) -> Option<Resource> {
    Some(match r {
        Rlimit::Core => libc::RLIMIT_CORE,
        Rlimit::Data => libc::RLIMIT_DATA,
        Rlimit::FileSize => libc::RLIMIT_FSIZE,
        Rlimit::MemLock => libc::RLIMIT_MEMLOCK,
        Rlimit::Rss => libc::RLIMIT_RSS,
        Rlimit::NoFile => libc::RLIMIT_NOFILE,
        Rlimit::Stack => libc::RLIMIT_STACK,
        Rlimit::Cpu => libc::RLIMIT_CPU,
        Rlimit::NProc => libc::RLIMIT_NPROC,
        Rlimit::As => libc::RLIMIT_AS,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Rlimit::Nice => libc::RLIMIT_NICE,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Rlimit::SigPending => libc::RLIMIT_SIGPENDING,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Rlimit::MsgQueue => libc::RLIMIT_MSGQUEUE,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Rlimit::RtPrio => libc::RLIMIT_RTPRIO,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Rlimit::RtTime => libc::RLIMIT_RTTIME,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Rlimit::Locks => libc::RLIMIT_LOCKS,
        #[allow(unreachable_patterns)]
        _ => return None,
    })
}

// `rlim_t` is 64 bits on the platforms built for, narrower on some 32-bit ones.
#[allow(clippy::useless_conversion)]
fn from_raw(v: libc::rlim_t) -> Limit {
    if v == libc::RLIM_INFINITY {
        None
    } else {
        Some(u64::from(v))
    }
}

/// `None` (unlimited) as `RLIM_INFINITY`; values past what the type holds too.
#[allow(clippy::useless_conversion)]
pub(crate) fn to_raw(l: Limit) -> libc::rlim_t {
    match l {
        None => libc::RLIM_INFINITY,
        Some(v) => libc::rlim_t::try_from(v).unwrap_or(libc::RLIM_INFINITY),
    }
}

/// The process's soft and hard limit.
pub(crate) fn get(r: Rlimit) -> Option<(Limit, Limit)> {
    let res = resource(r)?;
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `lim` is a valid, writable rlimit; getrlimit only writes it.
    if unsafe { libc::getrlimit(res, &raw mut lim) } != 0 {
        return None;
    }
    Some((from_raw(lim.rlim_cur), from_raw(lim.rlim_max)))
}

/// The limits a child starts with, ready for `setrlimit` after the fork.
pub(crate) fn for_child(limits: &[(Rlimit, Limit, Limit)]) -> Vec<(Resource, libc::rlimit)> {
    limits
        .iter()
        .filter_map(|&(r, soft, hard)| {
            Some((
                resource(r)?,
                libc::rlimit {
                    rlim_cur: to_raw(soft),
                    rlim_max: to_raw(hard),
                },
            ))
        })
        .collect()
}

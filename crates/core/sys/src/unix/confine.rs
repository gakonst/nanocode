use std::{
    io,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
};

const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;
const LANDLOCK_RULE_NET_PORT: libc::c_int = 2;
const NO_FLAGS: u32 = 0;

#[repr(C)]
struct RulesetAttributes {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneathAttributes {
    allowed_access: u64,
    parent_fd: i32,
}

#[repr(C)]
struct NetPortAttributes {
    allowed_access: u64,
    port: u64,
}

pub fn landlock_abi_version() -> io::Result<i32> {
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttributes>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    } as libc::c_int;
    if version < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(version)
}

pub fn landlock_create_ruleset(
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
) -> io::Result<OwnedFd> {
    let attributes = RulesetAttributes {
        handled_access_fs,
        handled_access_net,
        scoped,
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &raw const attributes,
            size_of::<RulesetAttributes>(),
            NO_FLAGS,
        )
    } as libc::c_int;
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub fn landlock_add_path_beneath_rule(
    ruleset: impl AsFd,
    parent: impl AsFd,
    allowed_access: u64,
) -> io::Result<()> {
    let attributes = PathBeneathAttributes {
        allowed_access,
        parent_fd: parent.as_fd().as_raw_fd(),
    };
    let result = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset.as_fd().as_raw_fd(),
            LANDLOCK_RULE_PATH_BENEATH,
            &raw const attributes,
            NO_FLAGS,
        )
    } as libc::c_int;
    match result {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

pub fn landlock_add_net_port_rule(
    ruleset: impl AsFd,
    port: u64,
    allowed_access: u64,
) -> io::Result<()> {
    let attributes = NetPortAttributes {
        allowed_access,
        port,
    };
    let result = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset.as_fd().as_raw_fd(),
            LANDLOCK_RULE_NET_PORT,
            &raw const attributes,
            NO_FLAGS,
        )
    } as libc::c_int;
    match result {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

pub fn landlock_restrict_self(ruleset: impl AsFd, flags: u32) -> io::Result<()> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_landlock_restrict_self,
            ruleset.as_fd().as_raw_fd(),
            flags,
        )
    } as libc::c_int;
    match result {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

pub fn is_directory(fd: impl AsFd) -> io::Result<bool> {
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd.as_fd().as_raw_fd(), status.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let status = unsafe { status.assume_init() };
    Ok(status.st_mode & libc::S_IFMT == libc::S_IFDIR)
}

pub fn set_no_new_privileges() -> io::Result<()> {
    match unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

use std::{
    fmt,
    fs::OpenOptions,
    io,
    os::{
        fd::{AsFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
    path::Path,
};

pub(crate) const FS_EXECUTE: u64 = 1 << 0;
pub(crate) const FS_WRITE_FILE: u64 = 1 << 1;
pub(crate) const FS_READ_FILE: u64 = 1 << 2;
pub(crate) const FS_READ_DIR: u64 = 1 << 3;
const FS_REMOVE_DIR: u64 = 1 << 4;
const FS_REMOVE_FILE: u64 = 1 << 5;
const FS_MAKE_CHAR: u64 = 1 << 6;
const FS_MAKE_DIR: u64 = 1 << 7;
const FS_MAKE_REG: u64 = 1 << 8;
const FS_MAKE_SOCK: u64 = 1 << 9;
const FS_MAKE_FIFO: u64 = 1 << 10;
const FS_MAKE_BLOCK: u64 = 1 << 11;
const FS_MAKE_SYM: u64 = 1 << 12;
const FS_REFER: u64 = 1 << 13;
const FS_TRUNCATE: u64 = 1 << 14;
pub(crate) const FS_IOCTL_DEV: u64 = 1 << 15;
const FS_RESOLVE_UNIX: u64 = 1 << 16;
pub(crate) const NET_BIND_TCP: u64 = 1 << 0;
pub(crate) const NET_CONNECT_TCP: u64 = 1 << 1;
pub(crate) const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
pub(crate) const SCOPE_SIGNAL: u64 = 1 << 1;

const FS_READ: u64 = FS_EXECUTE | FS_READ_FILE | FS_READ_DIR;
const FS_WRITE_ABI_1: u64 = FS_WRITE_FILE
    | FS_REMOVE_DIR
    | FS_REMOVE_FILE
    | FS_MAKE_CHAR
    | FS_MAKE_DIR
    | FS_MAKE_REG
    | FS_MAKE_SOCK
    | FS_MAKE_FIFO
    | FS_MAKE_BLOCK
    | FS_MAKE_SYM;
const FS_FILE: u64 =
    FS_READ_FILE | FS_WRITE_FILE | FS_EXECUTE | FS_TRUNCATE | FS_IOCTL_DEV | FS_RESOLVE_UNIX;
const RESTRICT_SELF_FLAGS: u32 = 0;

const FS_NAMES: [&str; 17] = [
    "Execute",
    "WriteFile",
    "ReadFile",
    "ReadDir",
    "RemoveDir",
    "RemoveFile",
    "MakeChar",
    "MakeDir",
    "MakeReg",
    "MakeSock",
    "MakeFifo",
    "MakeBlock",
    "MakeSym",
    "Refer",
    "Truncate",
    "IoctlDev",
    "ResolveUnix",
];
const NET_NAMES: [&str; 2] = ["BindTcp", "ConnectTcp"];
const SCOPE_NAMES: [&str; 2] = ["AbstractUnixSocket", "Signal"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Abi {
    Unsupported,
    V1,
    V2,
    V3,
    V4,
    V5,
    V6,
    V7,
    V8,
    V9,
}

impl Abi {
    const fn from_version(version: i32) -> Self {
        match version {
            ..=0 => Self::Unsupported,
            1 => Self::V1,
            2 => Self::V2,
            3 => Self::V3,
            4 => Self::V4,
            5 => Self::V5,
            6 => Self::V6,
            7 => Self::V7,
            8 => Self::V8,
            _ => Self::V9,
        }
    }

    fn current() -> Self {
        nanocode_sys::unix::confine::landlock_abi_version()
            .map_or(Self::Unsupported, Self::from_version)
    }

    pub(crate) const fn fs_read(self) -> u64 {
        match self {
            Self::Unsupported => 0,
            _ => FS_READ,
        }
    }

    const fn fs_write(self) -> u64 {
        match self {
            Self::Unsupported => 0,
            Self::V1 => FS_WRITE_ABI_1,
            Self::V2 => FS_WRITE_ABI_1 | FS_REFER,
            Self::V3 | Self::V4 => FS_WRITE_ABI_1 | FS_REFER | FS_TRUNCATE,
            Self::V5 | Self::V6 | Self::V7 | Self::V8 => {
                FS_WRITE_ABI_1 | FS_REFER | FS_TRUNCATE | FS_IOCTL_DEV
            }
            Self::V9 => FS_WRITE_ABI_1 | FS_REFER | FS_TRUNCATE | FS_IOCTL_DEV | FS_RESOLVE_UNIX,
        }
    }

    pub(crate) const fn fs_all(self) -> u64 {
        self.fs_read() | self.fs_write()
    }

    const fn net_all(self) -> u64 {
        match self {
            Self::Unsupported | Self::V1 | Self::V2 | Self::V3 => 0,
            _ => NET_BIND_TCP | NET_CONNECT_TCP,
        }
    }

    const fn scope_all(self) -> u64 {
        match self {
            Self::Unsupported | Self::V1 | Self::V2 | Self::V3 | Self::V4 | Self::V5 => 0,
            _ => SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Fs,
    Net,
    Scope,
}

impl Kind {
    const fn all(self, abi: Abi) -> u64 {
        match self {
            Self::Fs => abi.fs_all(),
            Self::Net => abi.net_all(),
            Self::Scope => abi.scope_all(),
        }
    }

    const fn known(self) -> u64 {
        match self {
            Self::Fs => Abi::V9.fs_all(),
            Self::Net => Abi::V9.net_all(),
            Self::Scope => Abi::V9.scope_all(),
        }
    }

    const fn type_name(self) -> &'static str {
        match self {
            Self::Fs => "BitFlags<AccessFs>",
            Self::Net => "BitFlags<AccessNet>",
            Self::Scope => "BitFlags<Scope>",
        }
    }

    const fn names(self) -> &'static [&'static str] {
        match self {
            Self::Fs => &FS_NAMES,
            Self::Net => &NET_NAMES,
            Self::Scope => &SCOPE_NAMES,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Flags {
    kind: Kind,
    bits: u64,
}

impl fmt::Display for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({:#b}", self.kind.type_name(), self.bits)?;
        let names: Vec<&str> = self
            .kind
            .names()
            .iter()
            .enumerate()
            .filter(|(index, _)| self.bits & (1 << index) != 0)
            .map(|(_, name)| *name)
            .collect();
        if !names.is_empty() {
            write!(f, ", {}", names.join(" | "))?;
        }
        f.write_str(")")
    }
}

#[derive(Debug)]
pub(crate) enum LandlockError {
    Empty,
    Unknown(Flags),
    Incompatible(Flags),
    PartiallyCompatible(Flags),
    MissingHandledAccess,
    CreateRuleset(io::Error),
    UnhandledAccess(Flags),
    Stat(io::Error),
    DirectoryAccess(Flags),
    AddRule(io::Error),
    SetNoNewPrivileges(io::Error),
    RestrictSelf(io::Error),
}

impl fmt::Display for LandlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty access-right"),
            Self::Unknown(unknown) => {
                write!(f, "unknown access-rights (at build time): {unknown}")
            }
            Self::Incompatible(access) => write!(f, "fully incompatible access-rights: {access}"),
            Self::PartiallyCompatible(incompatible) => {
                write!(f, "partially incompatible access-rights: {incompatible}")
            }
            Self::MissingHandledAccess => f.write_str("missing access"),
            Self::CreateRuleset(source) => write!(f, "failed to create a ruleset: {source}"),
            Self::UnhandledAccess(incompatible) => {
                write!(f, "access-rights not handled by the ruleset: {incompatible}")
            }
            Self::Stat(source) => write!(f, "failed to check file descriptor type: {source}"),
            Self::DirectoryAccess(incompatible) => {
                write!(f, "incompatible directory-only access-rights: {incompatible}")
            }
            Self::AddRule(source) => write!(f, "failed to add a rule: {source}"),
            Self::SetNoNewPrivileges(source) => write!(f, "failed to set no_new_privs: {source}"),
            Self::RestrictSelf(source) => {
                write!(f, "failed to restrict the calling thread: {source}")
            }
        }
    }
}

impl std::error::Error for LandlockError {}

const fn required(kind: Kind, access: u64, abi: Abi) -> Result<u64, LandlockError> {
    if access == 0 {
        return Err(LandlockError::Empty);
    }
    let unknown = access & !kind.known();
    if unknown != 0 {
        return Err(LandlockError::Unknown(Flags { kind, bits: unknown }));
    }
    let compatible = access & kind.all(abi);
    if compatible == 0 {
        return Err(LandlockError::Incompatible(Flags { kind, bits: access }));
    }
    if compatible != access {
        let bits = access & !compatible;
        return Err(LandlockError::PartiallyCompatible(Flags { kind, bits }));
    }
    Ok(compatible)
}

fn is_file(fd: impl AsFd) -> io::Result<bool> {
    nanocode_sys::unix::confine::is_directory(fd).map(|directory| !directory)
}

pub(crate) struct Ruleset {
    abi: Abi,
    fs: u64,
    net: u64,
    scoped: u64,
}

impl Ruleset {
    pub(crate) fn new() -> Self {
        Self {
            abi: Abi::current(),
            fs: 0,
            net: 0,
            scoped: 0,
        }
    }

    pub(crate) fn handle(mut self, kind: Kind, access: u64) -> Result<Self, LandlockError> {
        let compatible = required(kind, access, self.abi)?;
        match kind {
            Kind::Fs => self.fs |= compatible,
            Kind::Net => self.net |= compatible,
            Kind::Scope => self.scoped |= compatible,
        }
        Ok(self)
    }

    pub(crate) fn create(self) -> Result<Created, LandlockError> {
        if self.fs == 0 && self.net == 0 && self.scoped == 0 {
            return Err(LandlockError::MissingHandledAccess);
        }
        let fd = nanocode_sys::unix::confine::landlock_create_ruleset(self.fs, self.net, self.scoped)
            .map_err(LandlockError::CreateRuleset)?;
        Ok(Created {
            fd,
            abi: self.abi,
            fs: self.fs,
            net: self.net,
        })
    }
}

pub(crate) struct Created {
    fd: OwnedFd,
    abi: Abi,
    fs: u64,
    net: u64,
}

impl Created {
    pub(crate) fn add_paths<P: AsRef<Path>>(
        &self,
        paths: impl IntoIterator<Item = P>,
        access: u64,
    ) -> Result<(), LandlockError> {
        for path in paths {
            let Ok(parent) = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_CLOEXEC)
                .open(path.as_ref())
            else {
                continue;
            };
            let access = match is_file(&parent) {
                Ok(true) => access & FS_FILE,
                Err(_) | Ok(false) => access,
            };
            self.add_path_beneath(&parent, access)?;
        }
        Ok(())
    }

    pub(crate) fn add_path_beneath(
        &self,
        parent: impl AsFd,
        access: u64,
    ) -> Result<(), LandlockError> {
        let unhandled = access & !self.fs;
        if unhandled != 0 {
            let flags = Flags {
                kind: Kind::Fs,
                bits: unhandled,
            };
            return Err(LandlockError::UnhandledAccess(flags));
        }
        let valid = match is_file(&parent).map_err(LandlockError::Stat)? {
            true => access & FS_FILE,
            false => access,
        };
        if valid != access {
            let flags = Flags {
                kind: Kind::Fs,
                bits: access ^ valid,
            };
            return Err(LandlockError::DirectoryAccess(flags));
        }
        let access = required(Kind::Fs, access, self.abi)?;
        nanocode_sys::unix::confine::landlock_add_path_beneath_rule(&self.fd, parent, access)
            .map_err(LandlockError::AddRule)
    }

    pub(crate) fn add_port(&self, port: u16, access: u64) -> Result<(), LandlockError> {
        let unhandled = access & !self.net;
        if unhandled != 0 {
            let flags = Flags {
                kind: Kind::Net,
                bits: unhandled,
            };
            return Err(LandlockError::UnhandledAccess(flags));
        }
        let access = required(Kind::Net, access, self.abi)?;
        nanocode_sys::unix::confine::landlock_add_net_port_rule(&self.fd, u64::from(port), access)
            .map_err(LandlockError::AddRule)
    }

    pub(crate) fn restrict_self(self) -> Result<(), LandlockError> {
        nanocode_sys::unix::confine::set_no_new_privileges()
            .map_err(LandlockError::SetNoNewPrivileges)?;
        nanocode_sys::unix::confine::landlock_restrict_self(&self.fd, RESTRICT_SELF_FLAGS)
            .map_err(LandlockError::RestrictSelf)
    }
}

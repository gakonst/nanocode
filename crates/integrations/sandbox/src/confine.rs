use crate::{
    SYSTEM,
    landlock::{
        Abi, Created, FS_IOCTL_DEV, FS_READ_DIR, FS_READ_FILE, FS_WRITE_FILE, Kind,
        NET_BIND_TCP, NET_CONNECT_TCP, Ruleset, SCOPE_ABSTRACT_UNIX_SOCKET, SCOPE_SIGNAL,
    },
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, sock_filter,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::File,
    io::{self, PipeWriter},
    os::{
        fd::{AsFd, AsRawFd, IntoRawFd, OwnedFd, RawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::{Path, PathBuf},
};
use tokio::process::Command;

const ABI: Abi = Abi::V6;

pub const ARGUMENT: &str = "tool-sandbox";

pub const CHANNEL: &str = "tool-channel";

pub const CHANNEL_DESCRIPTOR: RawFd = 3;

pub struct Channel {
    pub host: UnixStream,
    pub program: OwnedFd,
    pub input: PipeWriter,
}

pub fn channel() -> io::Result<Channel> {
    let (host, program) = UnixStream::pair()?;
    let (reader, input) = io::pipe()?;
    nanocode_sys::unix::send_descriptor(&host, reader.as_fd())?;
    Ok(Channel {
        host,
        program: program.into(),
        input,
    })
}

fn take_channel() -> io::Result<()> {
    let channel = nanocode_sys::unix::duplicate_close_on_exec(io::stdin(), CHANNEL_DESCRIPTOR)?;
    if channel.as_raw_fd() != CHANNEL_DESCRIPTOR {
        return Err(other("the channel's descriptor is taken"));
    }
    let input = nanocode_sys::unix::receive_descriptor(&channel)?
        .ok_or_else(|| other("no input came with the channel"))?;
    nanocode_sys::unix::redirect_input(&input)?;
    nanocode_sys::unix::set_close_on_exec(&channel, false)?;
    let _ = channel.into_raw_fd();
    Ok(())
}

const DEVICES: [&str; 5] = [
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub read: Vec<PathBuf>,
    pub write: PathBuf,
    pub port: u16,
}

fn other(error: impl Into<Box<dyn Error + Send + Sync>>) -> io::Error {
    io::Error::other(error)
}

pub fn check() -> io::Result<()> {
    ruleset()?;
    filter()?;
    Ok(())
}

fn ruleset() -> io::Result<Created> {
    Ruleset::new()
        .handle(Kind::Fs, ABI.fs_all())
        .and_then(|ruleset| ruleset.handle(Kind::Net, NET_BIND_TCP | NET_CONNECT_TCP))
        .and_then(|ruleset| ruleset.handle(Kind::Scope, SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL))
        .and_then(Ruleset::create)
        .map_err(|error| other(format!("Landlock ABI 6 (Linux 6.12) is required: {error}")))
}

fn filter() -> io::Result<BpfProgram> {
    let argument = |index, operation, value: i32| -> io::Result<SeccompCondition> {
        let value = u64::try_from(value).map_err(other)?;
        SeccompCondition::new(index, SeccompCmpArgLen::Dword, operation, value).map_err(other)
    };
    let rule = |conditions| SeccompRule::new(conditions).map_err(other);
    let mut socket = vec![
        rule(vec![
            argument(0, SeccompCmpOp::Ne, libc::AF_INET)?,
            argument(0, SeccompCmpOp::Ne, libc::AF_INET6)?,
        ])?,
        rule(vec![
            argument(2, SeccompCmpOp::Ne, 0)?,
            argument(2, SeccompCmpOp::Ne, libc::IPPROTO_TCP)?,
        ])?,
    ];
    for kind in (0..16).filter(|kind| *kind != libc::SOCK_STREAM) {
        socket.push(rule(vec![argument(1, SeccompCmpOp::MaskedEq(0xf), kind)?])?);
    }
    let fast_open_mask = u64::try_from(libc::MSG_FASTOPEN).map_err(other)?;
    let fast_open = |index| -> io::Result<Vec<SeccompRule>> {
        Ok(vec![rule(vec![argument(
            index,
            SeccompCmpOp::MaskedEq(fast_open_mask),
            libc::MSG_FASTOPEN,
        )?])?])
    };
    let rules = BTreeMap::from([
        (libc::SYS_socket, socket),
        (libc::SYS_sendto, fast_open(3)?),
        (libc::SYS_sendmsg, fast_open(2)?),
        (libc::SYS_sendmmsg, fast_open(3)?),
        (libc::SYS_io_uring_setup, Vec::new()),
        (libc::SYS_io_uring_enter, Vec::new()),
        (libc::SYS_io_uring_register, Vec::new()),
        (libc::SYS_listen, Vec::new()),
        (libc::SYS_accept, Vec::new()),
        (libc::SYS_accept4, Vec::new()),
        (libc::SYS_setsid, Vec::new()),
        (libc::SYS_setpgid, Vec::new()),
    ]);
    let errno = u32::try_from(libc::EPERM).map_err(other)?;
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(errno),
        std::env::consts::ARCH.try_into().map_err(other)?,
    )
    .map_err(other)?;
    let mut program: BpfProgram = filter.try_into().map_err(other)?;
    if cfg!(target_arch = "x86_64") {
        let instruction = |code: u32, jt, jf, k| -> io::Result<sock_filter> {
            Ok(sock_filter {
                code: u16::try_from(code).map_err(other)?,
                jt,
                jf,
                k,
            })
        };
        program.splice(
            0..0,
            [
                instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0)?,
                instruction(
                    libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K,
                    0,
                    1,
                    0x4000_0000,
                )?,
                instruction(
                    libc::BPF_RET | libc::BPF_K,
                    0,
                    0,
                    libc::SECCOMP_RET_ERRNO | errno,
                )?,
            ],
        );
    }
    Ok(program)
}

fn confine(policy: &Policy, own: &File) -> io::Result<()> {
    let read = ABI.fs_read();
    let device = FS_READ_FILE | FS_WRITE_FILE | FS_IOCTL_DEV;
    let ruleset = ruleset()?;
    ruleset
        .add_paths(SYSTEM, read)
        .and_then(|()| ruleset.add_paths(&policy.read, read))
        .and_then(|()| ruleset.add_paths(["/"], FS_READ_DIR))
        .and_then(|()| ruleset.add_path_beneath(own, FS_READ_FILE))
        .and_then(|()| ruleset.add_paths(DEVICES, device))
        .and_then(|()| ruleset.add_paths([&policy.write], ABI.fs_all()))
        .and_then(|()| ruleset.add_port(policy.port, NET_CONNECT_TCP))
        .and_then(|()| ruleset.restrict_self())
        .map_err(other)?;
    seccompiler::apply_filter(&filter()?).map_err(other)?;
    Ok(())
}

impl Policy {
    pub fn command(&self, launcher: &Path, program: &Path) -> io::Result<Command> {
        self.launched(Command::new(launcher), program)
    }

    pub fn command_with_channel(&self, launcher: &Path, program: &Path) -> io::Result<Command> {
        let mut command = Command::new(launcher);
        command.arg(CHANNEL);
        self.launched(command, program)
    }

    fn launched(&self, mut command: Command, program: &Path) -> io::Result<Command> {
        command
            .arg(ARGUMENT)
            .arg(serde_json::to_string(self)?)
            .arg(program)
            .current_dir(&self.write)
            .process_group(0)
            .env_clear()
            .env("HOME", &self.write)
            .env("TMPDIR", &self.write)
            .kill_on_drop(true);
        Ok(command)
    }
}

pub fn launch_if_requested() {
    let mut arguments = std::env::args_os().skip(1).peekable();
    let channel = arguments.next_if(|first| first == CHANNEL).is_some();
    let confined = arguments.next_if(|first| first == ARGUMENT).is_some();
    if !channel && !confined {
        return;
    }
    let error = (|| -> io::Result<io::Error> {
        if channel {
            take_channel()?;
        }
        if confined {
            let policy = arguments.next().ok_or_else(|| other("no policy"))?;
            let policy = policy
                .to_str()
                .ok_or_else(|| other("policy is not UTF-8"))?;
            let policy: Policy = serde_json::from_str(policy)?;
            let own = File::open(format!("/proc/{}", std::process::id()))?;
            nanocode_sys::unix::set_close_on_exec(&own, false)?;
            confine(&policy, &own).map_err(|error| other(format!("confining: {error}")))?;
        }
        let program = arguments.next().ok_or_else(|| other("no program"))?;
        Ok(std::process::Command::new(program).args(arguments).exec())
    })();
    match error {
        Ok(error) | Err(error) => eprintln!("{ARGUMENT}: {error}"),
    }
    std::process::exit(126);
}

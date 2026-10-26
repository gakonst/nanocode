use crate::SYSTEM;
use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, PathBeneath, Ruleset,
    RulesetAttr, RulesetCreated, RulesetCreatedAttr, Scope, path_beneath_rules,
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
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
};
use tokio::process::Command;

const ABI: ABI = ABI::V6;

pub const ARGUMENT: &str = "tool-sandbox";

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

fn ruleset() -> io::Result<RulesetCreated> {
    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(ABI))
        .and_then(|ruleset| ruleset.handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp))
        .and_then(|ruleset| ruleset.scope(Scope::AbstractUnixSocket | Scope::Signal))
        .and_then(|ruleset| ruleset.create())
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
    let read = AccessFs::from_read(ABI);
    let device = AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::IoctlDev;
    ruleset()?
        .add_rules(path_beneath_rules(SYSTEM, read))
        .and_then(|ruleset| ruleset.add_rules(path_beneath_rules(&policy.read, read)))
        .and_then(|ruleset| ruleset.add_rules(path_beneath_rules(["/"], AccessFs::ReadDir)))
        .and_then(|ruleset| ruleset.add_rule(PathBeneath::new(own, AccessFs::ReadFile)))
        .and_then(|ruleset| ruleset.add_rules(path_beneath_rules(DEVICES, device)))
        .and_then(|ruleset| {
            ruleset.add_rules(path_beneath_rules([&policy.write], AccessFs::from_all(ABI)))
        })
        .and_then(|ruleset| ruleset.add_rule(NetPort::new(policy.port, AccessNet::ConnectTcp)))
        .and_then(|ruleset| ruleset.restrict_self())
        .map_err(other)?;
    seccompiler::apply_filter(&filter()?).map_err(other)?;
    Ok(())
}

impl Policy {
    pub fn command(&self, launcher: &Path, program: &Path) -> io::Result<Command> {
        let mut command = Command::new(launcher);
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
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().is_none_or(|first| first != ARGUMENT) {
        return;
    }
    let error = (|| -> io::Result<io::Error> {
        let policy = arguments.next().ok_or_else(|| other("no policy"))?;
        let policy = policy
            .to_str()
            .ok_or_else(|| other("policy is not UTF-8"))?;
        let policy: Policy = serde_json::from_str(policy)?;
        let program = arguments.next().ok_or_else(|| other("no program"))?;
        let own = File::open(format!("/proc/{}", std::process::id()))?;
        rustix::io::fcntl_setfd(&own, rustix::io::FdFlags::empty())?;
        confine(&policy, &own).map_err(|error| other(format!("confining: {error}")))?;
        Ok(std::process::Command::new(program).args(arguments).exec())
    })();
    match error {
        Ok(error) | Err(error) => eprintln!("{ARGUMENT}: {error}"),
    }
    std::process::exit(126);
}

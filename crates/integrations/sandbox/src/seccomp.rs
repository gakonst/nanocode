use nanocode_sys::unix::confine::SocketFilter;
use std::{collections::BTreeMap, fmt};

const BPF_LD: u16 = 0x00;
const BPF_ALU: u16 = 0x04;
const BPF_JMP: u16 = 0x05;
const BPF_RET: u16 = 0x06;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_AND: u16 = 0x50;
const BPF_JA: u16 = 0x00;
const BPF_JEQ: u16 = 0x10;
const BPF_JGE: u16 = 0x30;
const BPF_K: u16 = 0x00;

const SECCOMP_DATA_NUMBER_OFFSET: u32 = 0;
const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;
const SECCOMP_DATA_ARGUMENTS_OFFSET: u8 = 16;
const SECCOMP_DATA_ARGUMENT_BYTES: u8 = 8;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_DATA: u32 = 0x0000_ffff;

const AUDIT_ARCH_64BIT: u32 = 0x8000_0000;
const AUDIT_ARCH_LITTLE_ENDIAN: u32 = 0x4000_0000;
const ELF_MACHINE_X86_64: u32 = 62;
const ELF_MACHINE_AARCH64: u32 = 183;
const ELF_MACHINE_RISCV: u32 = 243;

const ARGUMENT_INDEX_MAX: u8 = 5;
const CONDITION_INSTRUCTIONS_MAX: u8 = 6;
const PROGRAM_INSTRUCTIONS_MAX: usize = 4096;

const X32_SYSCALL_BIT: u32 = 0x4000_0000;
const SOCKET_TYPE_MASK: u64 = 0xf;
const SOCKET_TYPES: i32 = 16;
const SOCKET_DOMAIN_ARGUMENT: u8 = 0;
const SOCKET_TYPE_ARGUMENT: u8 = 1;
const SOCKET_PROTOCOL_ARGUMENT: u8 = 2;
const SOCKET_DEFAULT_PROTOCOL: i32 = 0;
const SENDTO_FLAGS_ARGUMENT: u8 = 3;
const SENDMSG_FLAGS_ARGUMENT: u8 = 2;
const SENDMMSG_FLAGS_ARGUMENT: u8 = 3;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SeccompError {
    EmptyRule,
    FilterTooLarge(usize),
    IdenticalActions,
    InvalidArgumentNumber,
    InvalidTargetArch(String),
}

impl fmt::Display for SeccompError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRule => f.write_str("The condition vector of a rule cannot be empty."),
            Self::FilterTooLarge(length) => write!(
                f,
                "The seccomp filter contains too many BPF instructions: {length}. Max length is {PROGRAM_INSTRUCTIONS_MAX}."
            ),
            Self::IdenticalActions => f.write_str("`match_action` and `mismatch_action` are equal."),
            Self::InvalidArgumentNumber => write!(
                f,
                "The seccomp rule contains an invalid argument index. Maximum index value: {ARGUMENT_INDEX_MAX}"
            ),
            Self::InvalidTargetArch(arch) => write!(f, "Invalid target arch: {arch}."),
        }
    }
}

impl std::error::Error for SeccompError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arch {
    X86_64,
    Aarch64,
    Riscv64,
}

impl Arch {
    pub(crate) fn from_name(name: &str) -> Result<Self, SeccompError> {
        match name.to_lowercase().as_str() {
            "x86_64" => Ok(Self::X86_64),
            "aarch64" => Ok(Self::Aarch64),
            "riscv64" => Ok(Self::Riscv64),
            _ => Err(SeccompError::InvalidTargetArch(name.to_owned())),
        }
    }

    const fn audit(self) -> u32 {
        let machine = match self {
            Self::X86_64 => ELF_MACHINE_X86_64,
            Self::Aarch64 => ELF_MACHINE_AARCH64,
            Self::Riscv64 => ELF_MACHINE_RISCV,
        };
        machine | AUDIT_ARCH_64BIT | AUDIT_ARCH_LITTLE_ENDIAN
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    NotEqual,
    MaskedEqual(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Condition {
    index: u8,
    operation: Operation,
    value: u64,
}

impl Condition {
    pub(crate) const fn new(
        index: u8,
        operation: Operation,
        value: u64,
    ) -> Result<Self, SeccompError> {
        if index > ARGUMENT_INDEX_MAX {
            return Err(SeccompError::InvalidArgumentNumber);
        }
        Ok(Self {
            index,
            operation,
            value,
        })
    }

    fn program(self, offset: u8) -> Vec<SocketFilter> {
        let low_offset = u32::from(SECCOMP_DATA_ARGUMENTS_OFFSET + self.index * SECCOMP_DATA_ARGUMENT_BYTES);
        let load = statement(BPF_LD | BPF_W | BPF_ABS, low_offset);
        let program = match self.operation {
            Operation::NotEqual => vec![
                load,
                jump(BPF_JMP | BPF_JEQ | BPF_K, self.value as u32, offset, 0),
            ],
            Operation::MaskedEqual(mask) => vec![
                load,
                statement(BPF_ALU | BPF_AND | BPF_K, mask as u32),
                jump(BPF_JMP | BPF_JEQ | BPF_K, (self.value & mask) as u32, 0, offset),
            ],
        };
        assert!(program.len() <= usize::from(CONDITION_INSTRUCTIONS_MAX));
        program
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rule(Vec<Condition>);

impl Rule {
    pub(crate) fn new(conditions: Vec<Condition>) -> Result<Self, SeccompError> {
        if conditions.is_empty() {
            return Err(SeccompError::EmptyRule);
        }
        Ok(Self(conditions))
    }

    fn program(&self) -> Vec<SocketFilter> {
        let mut groups = Vec::with_capacity(self.0.len() + 1);
        let mut offset: u8 = 1;
        for condition in &self.0 {
            if offset.checked_add(CONDITION_INSTRUCTIONS_MAX + 1).is_none() {
                groups.push(vec![
                    statement(BPF_JMP | BPF_JA, 2),
                    statement(BPF_JMP | BPF_JA, u32::from(offset) + 1),
                    statement(BPF_JMP | BPF_JA, u32::from(offset) + 1),
                ]);
                offset = 1;
            }
            let program = condition.program(offset);
            offset += u8::try_from(program.len()).expect("a condition is at most six instructions");
            groups.push(program);
        }
        groups.push(vec![
            statement(BPF_JMP | BPF_JA, 1),
            statement(BPF_JMP | BPF_JA, u32::from(offset) + 1),
        ]);
        groups.into_iter().rev().flatten().collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Allow,
    Errno(u32),
}

impl Action {
    const fn value(self) -> u32 {
        match self {
            Self::Allow => SECCOMP_RET_ALLOW,
            Self::Errno(errno) => SECCOMP_RET_ERRNO | (errno & SECCOMP_RET_DATA),
        }
    }
}

const fn statement(code: u16, k: u32) -> SocketFilter {
    SocketFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

const fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SocketFilter {
    SocketFilter { code, jt, jf, k }
}

pub(crate) fn compile(
    rules: &BTreeMap<i64, Vec<Rule>>,
    mismatch: Action,
    matched: Action,
    arch: Arch,
) -> Result<Vec<SocketFilter>, SeccompError> {
    if mismatch == matched {
        return Err(SeccompError::IdenticalActions);
    }
    let mut program = vec![
        statement(BPF_LD | BPF_W | BPF_ABS, SECCOMP_DATA_ARCH_OFFSET),
        jump(BPF_JMP | BPF_JEQ | BPF_K, arch.audit(), 1, 0),
        statement(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
    ];
    if rules.is_empty() {
        program.push(statement(BPF_RET | BPF_K, mismatch.value()));
        return Ok(program);
    }
    program.push(statement(BPF_LD | BPF_W | BPF_ABS, SECCOMP_DATA_NUMBER_OFFSET));
    for (number, chain) in rules {
        let number = u32::try_from(*number).expect("a system call number fits in 32 bits");
        program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, number, 0, 1));
        if chain.is_empty() {
            program.push(statement(BPF_JMP | BPF_JA, 1));
            program.push(statement(BPF_JMP | BPF_JA, 2));
            program.push(statement(BPF_RET | BPF_K, matched.value()));
        }
        for rule in chain {
            program.extend(rule.program());
            program.push(statement(BPF_RET | BPF_K, matched.value()));
        }
        program.push(statement(BPF_RET | BPF_K, mismatch.value()));
    }
    program.push(statement(BPF_RET | BPF_K, mismatch.value()));
    if program.len() >= PROGRAM_INSTRUCTIONS_MAX {
        return Err(SeccompError::FilterTooLarge(program.len()));
    }
    Ok(program)
}

pub(crate) struct Syscalls {
    pub(crate) socket: i64,
    pub(crate) sendto: i64,
    pub(crate) sendmsg: i64,
    pub(crate) sendmmsg: i64,
    pub(crate) io_uring_setup: i64,
    pub(crate) io_uring_enter: i64,
    pub(crate) io_uring_register: i64,
    pub(crate) listen: i64,
    pub(crate) accept: i64,
    pub(crate) accept4: i64,
    pub(crate) setsid: i64,
    pub(crate) setpgid: i64,
}

pub(crate) const NATIVE: Syscalls = Syscalls {
    socket: libc::SYS_socket,
    sendto: libc::SYS_sendto,
    sendmsg: libc::SYS_sendmsg,
    sendmmsg: libc::SYS_sendmmsg,
    io_uring_setup: libc::SYS_io_uring_setup,
    io_uring_enter: libc::SYS_io_uring_enter,
    io_uring_register: libc::SYS_io_uring_register,
    listen: libc::SYS_listen,
    accept: libc::SYS_accept,
    accept4: libc::SYS_accept4,
    setsid: libc::SYS_setsid,
    setpgid: libc::SYS_setpgid,
};

fn argument(index: u8, operation: Operation, value: i32) -> Result<Condition, Box<dyn std::error::Error + Send + Sync>> {
    Ok(Condition::new(index, operation, u64::try_from(value)?)?)
}

pub(crate) fn tool_filter(
    arch_name: &str,
    syscalls: &Syscalls,
) -> Result<Vec<SocketFilter>, Box<dyn std::error::Error + Send + Sync>> {
    let mut socket = vec![
        Rule::new(vec![
            argument(SOCKET_DOMAIN_ARGUMENT, Operation::NotEqual, libc::AF_INET)?,
            argument(SOCKET_DOMAIN_ARGUMENT, Operation::NotEqual, libc::AF_INET6)?,
        ])?,
        Rule::new(vec![
            argument(SOCKET_PROTOCOL_ARGUMENT, Operation::NotEqual, SOCKET_DEFAULT_PROTOCOL)?,
            argument(SOCKET_PROTOCOL_ARGUMENT, Operation::NotEqual, libc::IPPROTO_TCP)?,
        ])?,
    ];
    for kind in (0..SOCKET_TYPES).filter(|kind| *kind != libc::SOCK_STREAM) {
        let masked = Operation::MaskedEqual(SOCKET_TYPE_MASK);
        socket.push(Rule::new(vec![argument(SOCKET_TYPE_ARGUMENT, masked, kind)?])?);
    }
    let fast_open_mask = u64::try_from(libc::MSG_FASTOPEN)?;
    let fast_open = |index| -> Result<Vec<Rule>, Box<dyn std::error::Error + Send + Sync>> {
        let masked = Operation::MaskedEqual(fast_open_mask);
        Ok(vec![Rule::new(vec![argument(index, masked, libc::MSG_FASTOPEN)?])?])
    };
    let rules = BTreeMap::from([
        (syscalls.socket, socket),
        (syscalls.sendto, fast_open(SENDTO_FLAGS_ARGUMENT)?),
        (syscalls.sendmsg, fast_open(SENDMSG_FLAGS_ARGUMENT)?),
        (syscalls.sendmmsg, fast_open(SENDMMSG_FLAGS_ARGUMENT)?),
        (syscalls.io_uring_setup, Vec::new()),
        (syscalls.io_uring_enter, Vec::new()),
        (syscalls.io_uring_register, Vec::new()),
        (syscalls.listen, Vec::new()),
        (syscalls.accept, Vec::new()),
        (syscalls.accept4, Vec::new()),
        (syscalls.setsid, Vec::new()),
        (syscalls.setpgid, Vec::new()),
    ]);
    let errno = u32::try_from(libc::EPERM)?;
    let arch = Arch::from_name(arch_name)?;
    let mut program = compile(&rules, Action::Allow, Action::Errno(errno), arch)?;
    if arch == Arch::X86_64 {
        program.splice(
            0..0,
            [
                statement(BPF_LD | BPF_W | BPF_ABS, SECCOMP_DATA_NUMBER_OFFSET),
                jump(BPF_JMP | BPF_JGE | BPF_K, X32_SYSCALL_BIT, 0, 1),
                statement(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | errno),
            ],
        );
    }
    Ok(program)
}

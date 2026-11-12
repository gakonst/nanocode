//! `trap`, the traps the engine runs (EXIT, ERR, DEBUG, RETURN, signals) and the
//! signal table (D1283).
//!
//! Signal numbers are Linux's on every platform, as the goldens and `kill -l` show
//! them; hosts map them to the platform's own (`host::Signal`).

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::state::{R, Unwind, Vp};

use super::{out, s};

/// The signals by number, without `SIG`; 32 and 33 are not shown by bash.
const NAMES: [&str; 32] = [
    "", "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "BUS", "FPE", "KILL", "USR1", "SEGV", "USR2",
    "PIPE", "ALRM", "TERM", "STKFLT", "CHLD", "CONT", "STOP", "TSTP", "TTIN", "TTOU", "URG",
    "XCPU", "XFSZ", "VTALRM", "PROF", "WINCH", "IO", "PWR", "SYS",
];
const RTMIN: i32 = 34;
const RTMAX: i32 = 64;

/// The name of a signal (`SIGHUP`, `SIGRTMIN+3`), or `None` for no signal.
pub(crate) fn signal_name(n: i32) -> Option<String> {
    if (1..32).contains(&n) {
        return Some(format!("SIG{}", NAMES[n as usize]));
    }
    if (RTMIN..=RTMAX).contains(&n) {
        let mid = RTMIN + (RTMAX - RTMIN) / 2;
        return Some(if n == RTMIN {
            "SIGRTMIN".into()
        } else if n == RTMAX {
            "SIGRTMAX".into()
        } else if n <= mid {
            format!("SIGRTMIN+{}", n - RTMIN)
        } else {
            format!("SIGRTMAX-{}", RTMAX - n)
        });
    }
    None
}

/// A signal by name (`INT`, `SIGINT`, `int`) or number.
pub(crate) fn signal_number(spec: &str) -> Option<i32> {
    if let Ok(n) = spec.parse::<i32>() {
        return signal_name(n).map(|_| n);
    }
    let up = spec.to_ascii_uppercase();
    let name = up.strip_prefix("SIG").unwrap_or(&up);
    if let Some(i) = NAMES.iter().position(|n| !n.is_empty() && *n == name) {
        return Some(i as i32);
    }
    (RTMIN..=RTMAX).find(|&n| signal_name(n).is_some_and(|s| s[3..] == *name))
}

/// What a trap is set on, in the order `trap -p` lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TrapKey {
    /// `EXIT` (signal 0).
    Exit,
    Signal(i32),
    Debug,
    Err,
    Return,
}

impl TrapKey {
    fn parse(spec: &[u8]) -> Option<TrapKey> {
        let spec = std::str::from_utf8(spec).ok()?;
        let up = spec.to_ascii_uppercase();
        Some(match up.as_str() {
            "EXIT" | "SIGEXIT" | "0" => TrapKey::Exit,
            "ERR" | "SIGERR" => TrapKey::Err,
            "DEBUG" | "SIGDEBUG" => TrapKey::Debug,
            "RETURN" | "SIGRETURN" => TrapKey::Return,
            _ => TrapKey::Signal(signal_number(spec)?),
        })
    }

    fn name(self) -> String {
        match self {
            TrapKey::Exit => "EXIT".into(),
            TrapKey::Err => "ERR".into(),
            TrapKey::Debug => "DEBUG".into(),
            TrapKey::Return => "RETURN".into(),
            TrapKey::Signal(n) => signal_name(n).unwrap_or_default(),
        }
    }
}

/// A trap: its command text (empty: ignore the signal), and whether it runs here. In a
/// subshell the parent's traps are still listed by `trap` (bash) but do not run, except
/// ignored signals and, with errtrace or functrace, ERR or DEBUG and RETURN.
#[derive(Clone, Debug)]
pub(crate) struct Trap {
    pub action: Rc<[u8]>,
    pub active: bool,
}

/// A vproc's traps.
#[derive(Clone, Debug, Default)]
pub(crate) struct Traps {
    pub map: BTreeMap<TrapKey, Trap>,
}

impl Traps {
    /// The action that runs for `key` here, if any.
    pub(crate) fn action(&self, key: TrapKey) -> Option<Rc<[u8]>> {
        self.map
            .get(&key)
            .filter(|t| t.active)
            .map(|t| t.action.clone())
    }

    /// The traps of a subshell (D1283).
    pub(crate) fn for_subshell(self: &Rc<Traps>, errtrace: bool, functrace: bool) -> Rc<Traps> {
        let keep = |k: TrapKey, t: &Trap| {
            t.action.is_empty()
                || (errtrace && k == TrapKey::Err)
                || (functrace && matches!(k, TrapKey::Debug | TrapKey::Return))
        };
        if self.map.iter().all(|(k, t)| !t.active || keep(*k, t)) {
            return self.clone();
        }
        let mut t = (**self).clone();
        for (k, trap) in t.map.iter_mut() {
            trap.active &= keep(*k, trap);
        }
        Rc::new(t)
    }
}

/// Runs a trap action in this vproc with `$?` set to `status`, and restores `$?`
/// after it (D1283). `exit` in the action ends the shell as usual.
pub(crate) async fn run_action(vp: &mut Vp, action: &[u8], status: i32) -> Result<(), Unwind> {
    let saved_in = vp.in_trap;
    let saved_line = vp.line;
    vp.in_trap = Some(status);
    vp.status = status;
    let line_base = vp.line.saturating_sub(1);
    let r = super::eval::run_text(vp, action, line_base, super::eval::Origin::Eval).await;
    vp.in_trap = saved_in;
    vp.line = saved_line;
    vp.status = status;
    match r {
        Ok(_) | Err(Unwind::Abort(_)) => Ok(()),
        Err(u) => Err(u),
    }
}

pub(crate) async fn run_action_status(vp: &mut Vp, action: &[u8], status: i32) -> R {
    let saved_in = vp.in_trap;
    let saved_line = vp.line;
    vp.in_trap = Some(status);
    vp.status = status;
    let line_base = vp.line.saturating_sub(1);
    let r = super::eval::run_text(vp, action, line_base, super::eval::Origin::Eval).await;
    let last = vp.status;
    vp.in_trap = saved_in;
    vp.line = saved_line;
    vp.status = status;
    match r {
        Ok(_) | Err(Unwind::Abort(_)) => Ok(last),
        Err(Unwind::Return(n)) => Ok(n),
        Err(u) => Err(u),
    }
}

/// The EXIT trap, when the (sub)shell ends with `status`: runs once; `exit N` inside it
/// gives the shell's status.
pub(crate) async fn exit_trap(vp: &mut Vp, status: i32) -> i32 {
    let Some(action) = vp.traps.action(TrapKey::Exit) else {
        return status;
    };
    Rc::make_mut(&mut vp.traps).map.remove(&TrapKey::Exit);
    match run_action(vp, &action, status).await {
        Err(Unwind::Exit(n)) => n,
        _ => status,
    }
}

/// The ERR trap after a failed command (the caller checked errexit's exemptions).
pub(crate) async fn err_trap(vp: &mut Vp, status: i32) -> Result<(), Unwind> {
    if vp.in_trap.is_some() {
        return Ok(());
    }
    match vp.traps.action(TrapKey::Err) {
        Some(action) if !action.is_empty() => run_action(vp, &action, status).await,
        _ => Ok(()),
    }
}

/// Runs the action of `key` (DEBUG, RETURN) if one is set and no trap is running.
pub(crate) async fn run_trap(vp: &mut Vp, key: TrapKey) -> Result<(), Unwind> {
    if vp.in_trap.is_some() {
        return Ok(());
    }
    match vp.traps.action(key) {
        Some(action) if !action.is_empty() => {
            let st = vp.status;
            run_action(vp, &action, st).await
        }
        _ => Ok(()),
    }
}

/// Traps a function call does not see (D1283): DEBUG and RETURN unless functrace, ERR
/// unless errtrace. Hidden on entry, restored on return unless the function set its own.
pub(crate) type Hidden = Vec<(TrapKey, Trap)>;

pub(crate) fn enter_function(vp: &mut Vp) -> Option<Hidden> {
    if vp.traps.map.is_empty() {
        return None;
    }
    let functrace = vp.opts.get(crate::options::SetOpt::Functrace)
        || vp.opts.shopt(crate::options::Shopt::Extdebug);
    let errtrace = vp.opts.get(crate::options::SetOpt::Errtrace);
    let hide = |k: &TrapKey| match k {
        TrapKey::Debug | TrapKey::Return => !functrace,
        TrapKey::Err => !errtrace,
        _ => false,
    };
    if !vp.traps.map.keys().any(hide) {
        return None;
    }
    let traps = Rc::make_mut(&mut vp.traps);
    let keys: Vec<TrapKey> = traps.map.keys().copied().filter(hide).collect();
    Some(
        keys.into_iter()
            .filter_map(|k| traps.map.remove(&k).map(|t| (k, t)))
            .collect(),
    )
}

/// After a function body: the RETURN trap the function sees, then the hidden traps come
/// back where the function did not set its own.
pub(crate) async fn leave_function(vp: &mut Vp, hidden: Option<Hidden>) -> Result<(), Unwind> {
    let r = run_trap(vp, TrapKey::Return).await;
    if let Some(hidden) = hidden {
        let traps = Rc::make_mut(&mut vp.traps);
        for (k, t) in hidden {
            traps.map.entry(k).or_insert(t);
        }
    }
    r
}

/// Signals the shell takes by default: they end it (with the EXIT trap) as `128+n`.
pub(crate) fn terminates(sig: i32) -> bool {
    !matches!(sig, 17 | 18 | 19 | 20 | 21 | 22 | 23 | 28)
}

/// Delivers pending signals to the shell (D1284): at a safepoint of the vproc that is
/// the shell itself. A trap runs its action; an ignored signal does nothing; otherwise
/// the default action ends the shell with `128+n` after the EXIT trap.
pub(crate) async fn take_signals(vp: &mut Vp) -> Result<(), Unwind> {
    if !vp.top || vp.in_trap.is_some() {
        return Ok(());
    }
    let pending = vp.sigs.take(&vp.core);
    if pending == 0 {
        return Ok(());
    }
    for sig in 1..64 {
        if pending & (1u64 << sig) == 0 {
            continue;
        }
        match vp.traps.action(TrapKey::Signal(sig)) {
            Some(action) if action.is_empty() => {}
            Some(action) => {
                let st = vp.status;
                run_action(vp, &action, st).await?;
            }
            None if terminates(sig) => {
                vp.sigs.died_of(&vp.core, sig);
                return Err(Unwind::Exit(128 + sig));
            }
            None => {}
        }
    }
    Ok(())
}

const USAGE: &str = "trap: usage: trap [-Plp] [[action] signal_spec ...]";

pub(crate) async fn trap(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut i = 1;
    let (mut list, mut print, mut print_action) = (false, false, false);
    while let Some(a) = args.get(i) {
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        // `trap -1 …`? A negative number is not an option; bash takes `-` alone as reset.
        for &c in &a[1..] {
            match c {
                b'l' => list = true,
                b'p' => print = true,
                b'P' => print_action = true,
                _ => {
                    vp.error(&format!("trap: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(format!("{USAGE}\n").as_bytes()).await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let rest = &args[i..];
    if list {
        return out(vp, "trap", &signal_list()).await;
    }
    if print_action {
        if rest.is_empty() {
            vp.error("trap: -P requires at least one signal name").await;
            return Ok(2);
        }
        let mut buf = Vec::new();
        let mut status = 0;
        for spec in rest {
            match TrapKey::parse(spec) {
                Some(k) => {
                    if let Some(t) = vp.traps.map.get(&k) {
                        buf.extend_from_slice(&t.action);
                        buf.push(b'\n');
                    }
                }
                None => {
                    status = invalid_spec(vp, spec).await;
                }
            }
        }
        let st = out(vp, "trap", &buf).await?;
        return Ok(if st != 0 { st } else { status });
    }
    if rest.is_empty() || print {
        let mut buf = Vec::new();
        let mut status = 0;
        let keys: Vec<TrapKey> = if rest.is_empty() {
            vp.traps.map.keys().copied().collect()
        } else {
            let mut keys = Vec::new();
            for spec in rest {
                match TrapKey::parse(spec) {
                    Some(k) => keys.push(k),
                    None => status = invalid_spec(vp, spec).await,
                }
            }
            keys
        };
        for k in keys {
            if let Some(t) = vp.traps.map.get(&k) {
                buf.extend_from_slice(b"trap -- ");
                buf.extend_from_slice(&single_quote(&t.action));
                buf.push(b' ');
                buf.extend_from_slice(k.name().as_bytes());
                buf.push(b'\n');
            }
        }
        let st = out(vp, "trap", &buf).await?;
        return Ok(if st != 0 { st } else { status });
    }
    // `trap SIG…` with a first operand that is a signal number resets; `trap - SIG…`
    // resets; otherwise the first operand is the action.
    let (action, specs): (Option<&[u8]>, &[Vec<u8>]) = if rest[0] == b"-" {
        (None, &rest[1..])
    } else if rest.len() == 1 {
        // A lone signal spec resets it (`trap EXIT`, `trap 2`).
        if TrapKey::parse(&rest[0]).is_some() {
            (None, rest)
        } else {
            vp.error_raw(format!("{USAGE}\n").as_bytes()).await;
            return Ok(2);
        }
    } else if std::str::from_utf8(&rest[0])
        .ok()
        .is_some_and(|s| s.parse::<u32>().is_ok())
    {
        (None, rest)
    } else {
        (Some(&rest[0]), &rest[1..])
    };
    let mut status = 0;
    let traps = Rc::make_mut(&mut vp.traps);
    // The first change in a subshell: the parent's traps that do not run here go.
    traps.map.retain(|_, t| t.active);
    let mut bad = Vec::new();
    for spec in specs {
        match TrapKey::parse(spec) {
            Some(k) => match action {
                Some(a) => {
                    traps.map.insert(
                        k,
                        Trap {
                            action: a.into(),
                            active: true,
                        },
                    );
                }
                None => {
                    traps.map.remove(&k);
                }
            },
            None => bad.push(spec.clone()),
        }
    }
    for spec in bad {
        status = invalid_spec(vp, &spec).await;
    }
    Ok(status)
}

async fn invalid_spec(vp: &Vp, spec: &[u8]) -> i32 {
    vp.error(&format!("trap: {}: invalid signal specification", s(spec)))
        .await;
    1
}

/// `'…'` with `'\''` for quotes, as `trap -p` prints actions.
fn single_quote(a: &[u8]) -> Vec<u8> {
    let mut out = vec![b'\''];
    for &b in a {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// `trap -l`: ` 1) SIGHUP\t 2) SIGINT…`, five to a line.
fn signal_list() -> Vec<u8> {
    let nums: Vec<i32> = (1..32).chain(RTMIN..=RTMAX).collect();
    let mut buf = Vec::new();
    for (k, n) in nums.iter().enumerate() {
        buf.extend_from_slice(format!("{n:2}) {}", signal_name(*n).unwrap_or_default()).as_bytes());
        buf.push(if (k + 1) % 5 == 0 || k + 1 == nums.len() {
            b'\n'
        } else {
            b'\t'
        });
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testhost::TestHost;
    use crate::{Script, Shell, Step};

    fn run(src: &str) -> (i32, String) {
        let (st, out, _) = crate::run_on_test_host(src);
        (st, String::from_utf8_lossy(&out).into_owned())
    }

    /// Runs `src`, sends `sig` after a few safepoints, and runs it to the end.
    fn with_signal(src: &str, sig: i32) -> (i32, String) {
        let host = std::rc::Rc::new(TestHost::new());
        let mut shell = Shell::new(host.config());
        shell.start(Script::command(src));
        let control = shell.control();
        let mut sent = false;
        let status = loop {
            match shell.step(20) {
                Step::Done(s) => break s,
                Step::Yield | Step::Blocked => {
                    if !sent {
                        control.signal(sig);
                        sent = true;
                    }
                }
                Step::Idle => break -1,
            }
        };
        (status, String::from_utf8_lossy(&host.stdout()).into_owned())
    }

    #[test]
    fn signals_run_traps_or_end_the_shell() {
        let lp = "i=0; while ((i < 2000)); do ((i++)); done; echo end";
        let (st, out) = with_signal(&format!("trap 'echo got $?' USR1; {lp}"), 10);
        assert_eq!((st, out.as_str()), (0, "got 0\nend\n"));
        let (st, out) = with_signal(&format!("trap 'echo bye' EXIT; {lp}"), 15);
        assert_eq!((st, out.as_str()), (143, "bye\n"));
        let (st, out) = with_signal(&format!("trap '' TERM; {lp}"), 15);
        assert_eq!((st, out.as_str()), (0, "end\n"));
        let (st, out) = with_signal(&format!("trap 'echo term; exit 7' TERM; {lp}"), 15);
        assert_eq!((st, out.as_str()), (7, "term\n"));
    }

    #[test]
    fn exit_err_return_debug() {
        assert_eq!(run("trap 'echo e $?' EXIT; exit 3"), (3, "e 3\n".into()));
        assert_eq!(run("trap 'true; exit' EXIT; exit 3"), (3, String::new()));
        assert_eq!(
            run(
                "trap 'echo err $?' ERR; false; f() { false; echo inf; }; f; ! false; false || true"
            ),
            (0, "err 1\ninf\n".into())
        );
        assert_eq!(
            run("set -e; trap 'echo err' ERR; trap 'echo exit' EXIT; false; echo no"),
            (1, "err\nexit\n".into())
        );
        assert_eq!(
            run("f() { trap 'echo ret' RETURN; echo f; }; g() { echo g; }; f; g"),
            (0, "f\nret\ng\n".into())
        );
        assert_eq!(
            run("trap 'echo dbg' DEBUG; echo a; f() { echo b; }; f"),
            (0, "dbg\na\ndbg\nb\n".into())
        );
        assert_eq!(
            run("trap 'echo bye' EXIT; (trap); echo $(trap -p EXIT); (trap 'echo sub' EXIT; :)"),
            (
                0,
                "trap -- 'echo bye' EXIT\ntrap -- 'echo bye' EXIT\nsub\nbye\n".into()
            )
        );
        assert_eq!(
            run("trap 'echo x' INT; trap INT; trap -p"),
            (0, String::new())
        );
        assert_eq!(run("trap 'a' BOGUS; echo $?"), (0, "1\n".into()));
    }

    #[test]
    fn names_and_numbers() {
        assert_eq!(signal_number("INT"), Some(2));
        assert_eq!(signal_number("sigusr1"), Some(10));
        assert_eq!(signal_number("15"), Some(15));
        assert_eq!(signal_number("RTMIN+1"), Some(35));
        assert_eq!(signal_number("SIGRTMAX-1"), Some(63));
        assert_eq!(signal_number("BOGUS"), None);
        assert_eq!(signal_name(64).as_deref(), Some("SIGRTMAX"));
        assert_eq!(signal_name(49).as_deref(), Some("SIGRTMIN+15"));
        assert_eq!(signal_name(50).as_deref(), Some("SIGRTMAX-14"));
        let l = signal_list();
        assert!(l.starts_with(b" 1) SIGHUP\t 2) SIGINT\t 3) SIGQUIT\t 4) SIGILL\t 5) SIGTRAP\n"));
        assert!(l.ends_with(b"64) SIGRTMAX\n"));
    }
}

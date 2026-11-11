//! Shell options: `set -o` (with their one-letter forms) and `shopt`.
//!
//! Every vproc carries its own copy (`Vp::opts`): a subshell inherits the options, and
//! changes inside it stay there, as in bash.

use crate::state::{Var, Vars};

/// The options of `set` / `set -o`, in the order `set -o` lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetOpt {
    Allexport,
    Braceexpand,
    Emacs,
    Errexit,
    Errtrace,
    Functrace,
    Hashall,
    Histexpand,
    History,
    Ignoreeof,
    InteractiveComments,
    Keyword,
    Monitor,
    Noclobber,
    Noexec,
    Noglob,
    Nolog,
    Notify,
    Nounset,
    Onecmd,
    Physical,
    Pipefail,
    Posix,
    Privileged,
    Verbose,
    Vi,
    Xtrace,
}

use SetOpt::*;

/// Name, one-letter form, and the default of a non-interactive shell.
pub(crate) const SET_OPTS: &[(SetOpt, &str, Option<u8>, bool)] = &[
    (Allexport, "allexport", Some(b'a'), false),
    (Braceexpand, "braceexpand", Some(b'B'), true),
    (Emacs, "emacs", None, false),
    (Errexit, "errexit", Some(b'e'), false),
    (Errtrace, "errtrace", Some(b'E'), false),
    (Functrace, "functrace", Some(b'T'), false),
    (Hashall, "hashall", Some(b'h'), true),
    (Histexpand, "histexpand", Some(b'H'), false),
    (History, "history", None, false),
    (Ignoreeof, "ignoreeof", None, false),
    (InteractiveComments, "interactive-comments", None, true),
    (Keyword, "keyword", Some(b'k'), false),
    (Monitor, "monitor", Some(b'm'), false),
    (Noclobber, "noclobber", Some(b'C'), false),
    (Noexec, "noexec", Some(b'n'), false),
    (Noglob, "noglob", Some(b'f'), false),
    (Nolog, "nolog", None, false),
    (Notify, "notify", Some(b'b'), false),
    (Nounset, "nounset", Some(b'u'), false),
    (Onecmd, "onecmd", Some(b't'), false),
    (Physical, "physical", Some(b'P'), false),
    (Pipefail, "pipefail", None, false),
    (Posix, "posix", None, false),
    (Privileged, "privileged", Some(b'p'), false),
    (Verbose, "verbose", Some(b'v'), false),
    (Vi, "vi", None, false),
    (Xtrace, "xtrace", Some(b'x'), false),
];

/// The order of the letters in `$-`.
const DASH_ORDER: &[u8] = b"abefhkmnptuvxBCEHPT";

/// The options of `shopt`, in the order `shopt` lists them, with their defaults in a
/// non-interactive bash 5.3.
pub(crate) const SHOPTS: &[(&str, bool)] = &[
    ("array_expand_once", false),
    ("assoc_expand_once", false),
    ("autocd", false),
    ("bash_source_fullpath", false),
    ("cdable_vars", false),
    ("cdspell", false),
    ("checkhash", false),
    ("checkjobs", false),
    ("checkwinsize", true),
    ("cmdhist", true),
    ("compat31", false),
    ("compat32", false),
    ("compat40", false),
    ("compat41", false),
    ("compat42", false),
    ("compat43", false),
    ("compat44", false),
    ("complete_fullquote", true),
    ("direxpand", false),
    ("dirspell", false),
    ("dotglob", false),
    ("execfail", false),
    ("expand_aliases", false),
    ("extdebug", false),
    ("extglob", false),
    ("extquote", true),
    ("failglob", false),
    ("force_fignore", true),
    ("globasciiranges", true),
    ("globskipdots", true),
    ("globstar", false),
    ("gnu_errfmt", false),
    ("histappend", false),
    ("histreedit", false),
    ("histverify", false),
    ("hostcomplete", true),
    ("huponexit", false),
    ("inherit_errexit", false),
    ("interactive_comments", true),
    ("lastpipe", false),
    ("lithist", false),
    ("localvar_inherit", false),
    ("localvar_unset", false),
    ("login_shell", false),
    ("mailwarn", false),
    ("no_empty_cmd_completion", false),
    ("nocaseglob", false),
    ("nocasematch", false),
    ("noexpand_translation", false),
    ("nullglob", false),
    ("patsub_replacement", true),
    ("progcomp", true),
    ("progcomp_alias", false),
    ("promptvars", true),
    ("restricted_shell", false),
    ("shift_verbose", false),
    ("sourcepath", true),
    ("varredir_close", false),
    ("xpg_echo", false),
];

/// `shopt` options the engine looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shopt {
    Dotglob,
    ExpandAliases,
    Extdebug,
    Extglob,
    Failglob,
    Globskipdots,
    Globstar,
    InheritErrexit,
    Lastpipe,
    Nocaseglob,
    Nocasematch,
    Nullglob,
    PatsubReplacement,
    XpgEcho,
}

impl Shopt {
    fn name(self) -> &'static str {
        match self {
            Shopt::Dotglob => "dotglob",
            Shopt::ExpandAliases => "expand_aliases",
            Shopt::Extdebug => "extdebug",
            Shopt::Extglob => "extglob",
            Shopt::Globskipdots => "globskipdots",
            Shopt::Globstar => "globstar",
            Shopt::Nocaseglob => "nocaseglob",
            Shopt::Nocasematch => "nocasematch",
            Shopt::Failglob => "failglob",
            Shopt::InheritErrexit => "inherit_errexit",
            Shopt::Lastpipe => "lastpipe",
            Shopt::Nullglob => "nullglob",
            Shopt::PatsubReplacement => "patsub_replacement",
            Shopt::XpgEcho => "xpg_echo",
        }
    }
}

/// All options of a vproc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    set: u32,
    shopt: u64,
    /// The script runs as `-c` (`c` in `$-`), or from stdin (`s`).
    pub dash_c: bool,
    pub stdin: bool,
}

impl Default for Options {
    fn default() -> Options {
        let mut o = Options {
            set: 0,
            shopt: 0,
            dash_c: false,
            stdin: false,
        };
        for (opt, _, _, on) in SET_OPTS {
            o.set(*opt, *on);
        }
        for (i, (_, on)) in SHOPTS.iter().enumerate() {
            if *on {
                o.shopt |= 1 << i;
            }
        }
        o
    }
}

impl Options {
    pub(crate) fn get(&self, opt: SetOpt) -> bool {
        self.set & (1 << opt as u32) != 0
    }

    pub(crate) fn set(&mut self, opt: SetOpt, on: bool) {
        if on {
            self.set |= 1 << opt as u32;
        } else {
            self.set &= !(1 << opt as u32);
        }
        // `set -o vi` turns emacs off and the other way round.
        match (opt, on) {
            (Emacs, true) => self.set &= !(1 << Vi as u32),
            (Vi, true) => self.set &= !(1 << Emacs as u32),
            _ => {}
        }
    }

    pub(crate) fn shopt(&self, opt: Shopt) -> bool {
        self.shopt_at(shopt_index(opt.name()).expect("known shopt"))
    }

    pub(crate) fn shopt_at(&self, i: usize) -> bool {
        self.shopt & (1 << i) != 0
    }

    pub(crate) fn set_shopt_at(&mut self, i: usize, on: bool) {
        if on {
            self.shopt |= 1 << i;
        } else {
            self.shopt &= !(1 << i);
        }
    }

    /// `$-`.
    pub(crate) fn dash(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for &c in DASH_ORDER {
            if let Some(opt) = by_letter(c)
                && self.get(opt)
            {
                out.push(c);
            }
        }
        if self.dash_c {
            out.push(b'c');
        }
        if self.stdin {
            out.push(b's');
        }
        out
    }

    /// `SHELLOPTS`: the `set -o` options that are on, colon-separated.
    pub(crate) fn shellopts(&self) -> Vec<u8> {
        let names: Vec<&str> = SET_OPTS
            .iter()
            .filter(|(o, ..)| self.get(*o))
            .map(|(_, n, ..)| *n)
            .collect();
        names.join(":").into_bytes()
    }

    /// `BASHOPTS`: the `shopt` options that are on.
    pub(crate) fn bashopts(&self) -> Vec<u8> {
        let names: Vec<&str> = SHOPTS
            .iter()
            .enumerate()
            .filter(|(i, _)| self.shopt_at(*i))
            .map(|(_, (n, _))| *n)
            .collect();
        names.join(":").into_bytes()
    }
}

pub(crate) fn by_letter(c: u8) -> Option<SetOpt> {
    SET_OPTS
        .iter()
        .find(|(_, _, l, _)| *l == Some(c))
        .map(|(o, ..)| *o)
}

pub(crate) fn by_name(name: &[u8]) -> Option<SetOpt> {
    SET_OPTS
        .iter()
        .find(|(_, n, ..)| n.as_bytes() == name)
        .map(|(o, ..)| *o)
}

pub(crate) fn shopt_index(name: &str) -> Option<usize> {
    SHOPTS.iter().position(|(n, _)| *n == name)
}

/// How the next chunk of a script, `eval` text or sourced file is parsed: `shopt -s
/// extglob` takes effect from the next line on (D1043).
pub(crate) fn parser_options(opts: &Options) -> psh_syntax::Options {
    psh_syntax::Options {
        extglob: opts.shopt(Shopt::Extglob),
    }
}

/// Applies options kept elsewhere in the vproc: `Vp::expand_aliases`, and `SHELLOPTS`
/// and `BASHOPTS` (readonly, as in bash).
pub(crate) fn sync(vp: &mut crate::state::Vp) {
    vp.expand_aliases = vp.opts.shopt(Shopt::ExpandAliases);
    sync_vars(&mut vp.vars, &vp.opts);
}

/// Sets `SHELLOPTS` and `BASHOPTS` (readonly, as in bash) from the options.
/// The options a new shell starts with: the defaults, plus the ones its environment's
/// `SHELLOPTS` and `BASHOPTS` name, as bash takes them (an exported `SHELLOPTS` passes
/// `set -x` on to the shells a script starts). POSIX mode stays off (D1280).
pub(crate) fn inherited(vars: &Vars) -> Options {
    let mut o = Options::default();
    if let Some(list) = vars.value("SHELLOPTS") {
        for name in list.split(|&b| b == b':') {
            if let Some(opt) = by_name(name)
                && name != b"posix"
            {
                o.set(opt, true);
            }
        }
    }
    if let Some(list) = vars.value("BASHOPTS") {
        for name in list.split(|&b| b == b':') {
            if let Some(i) = std::str::from_utf8(name).ok().and_then(shopt_index) {
                o.set_shopt_at(i, true);
            }
        }
    }
    o
}

pub(crate) fn sync_vars(vars: &mut Vars, opts: &Options) {
    for (name, value) in [
        ("SHELLOPTS", opts.shellopts()),
        ("BASHOPTS", opts.bashopts()),
    ] {
        let exported = vars.get(name).is_some_and(|v| v.exported);
        vars.insert(
            name,
            Var {
                value: crate::vars::Value::Str(value),
                exported,
                readonly: true,
                ..Var::default()
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_dash() {
        let mut o = Options {
            dash_c: true,
            ..Options::default()
        };
        assert_eq!(o.dash(), b"hBc");
        assert_eq!(o.shellopts(), b"braceexpand:hashall:interactive-comments");
        o.set(Errexit, true);
        o.set(Nounset, true);
        o.set(Xtrace, true);
        o.set(Noclobber, true);
        o.set(Pipefail, true);
        assert_eq!(o.dash(), b"ehuxBCc");
        assert_eq!(
            o.shellopts(),
            b"braceexpand:errexit:hashall:interactive-comments:noclobber:nounset:pipefail:xtrace"
        );
        assert!(
            o.bashopts()
                .starts_with(b"checkwinsize:cmdhist:complete_fullquote:extquote")
        );
        assert!(!o.shopt(Shopt::Nullglob));
        assert_eq!(SHOPTS.len(), 59);
        assert!(SHOPTS.len() <= 64);
    }
}

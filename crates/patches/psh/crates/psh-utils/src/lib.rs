//! psh's in-process utilities (ARCH.md "psh-utils", D1080–D1083): implementations of
//! external programs (`head`, `cat`, `timeout`, ...) that run inside the shell, on the
//! engine's scheduler, natively and in wasm.
//!
//! Adding a utility: a module with `SPEC` (its options, D1082) and `async fn run(ctx,
//! argv) -> i32`, plus one entry in [`REGISTRY`] and one in [`SPECS`].

mod common;
mod fsutil;
mod modes;
pub mod opts;
pub mod search;
mod sum_hash;
mod textio;
mod timefmt;

mod cat;
mod env;
mod head;
mod sleep;
mod tail;
mod timeout;
mod wc;

mod basename;
mod dirname;
mod mkdir;
mod readlink;
mod realpath;
mod rmdir;
mod touch;

mod awk;
mod chmod;
mod fd;
mod grep;
// Formatted in ripgrep's style (rg/rustfmt.toml), not the workspace's, so the copied code stays a
// small diff from upstream.
#[rustfmt::skip]
mod rg;

mod comm;
mod cp;
mod cut;
mod ln;
mod mv;
mod nl;
mod paste;
mod rm;

mod du;
mod ls;
mod mktemp;
mod seq;
mod sort;
mod stat;
mod tee;
mod tr;
mod uniq;

mod find;
mod git;
mod jq;

mod codec;
mod dd;
mod dump;
mod gzip;
mod lines;
mod patch;
mod small;
mod tar;
mod tree;
mod xargs;
mod zip;

mod base64;
mod cmp;
mod date;
mod diff;
mod diff_algo;
mod sed;
mod sum;
mod which;

use psh_engine::util::{Mode, UtilEntry, UtilPolicy};

use common::util_fn;

/// Every utility, with its names, mode (D015) and group.
pub static REGISTRY: &[UtilEntry] = &[
    UtilEntry {
        names: &["awk", "gawk"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(awk::run),
    },
    UtilEntry {
        names: &["jq"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(jq::run),
    },
    UtilEntry {
        names: &["git"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(git::run),
    },
    UtilEntry {
        names: &["tar"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(tar::run),
    },
    UtilEntry {
        names: &["gzip", "gunzip", "zcat"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(gzip::run),
    },
    UtilEntry {
        names: &["zip"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(zip::zip),
    },
    UtilEntry {
        names: &["unzip"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(zip::unzip),
    },
    UtilEntry {
        names: &["patch"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(patch::run),
    },
    UtilEntry {
        names: &["xxd"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(dump::xxd),
    },
    UtilEntry {
        names: &["od"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(dump::od),
    },
    UtilEntry {
        names: &["hexdump"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(dump::hexdump),
    },
    UtilEntry {
        names: &["strings"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(dump::strings),
    },
    UtilEntry {
        names: &["base32"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(dump::base32),
    },
    UtilEntry {
        names: &["tac"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::tac),
    },
    UtilEntry {
        names: &["rev"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::rev),
    },
    UtilEntry {
        names: &["fold"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::fold),
    },
    UtilEntry {
        names: &["expand"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::expand),
    },
    UtilEntry {
        names: &["unexpand"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::unexpand),
    },
    UtilEntry {
        names: &["column"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::column),
    },
    UtilEntry {
        names: &["join"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::join),
    },
    UtilEntry {
        names: &["shuf"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::shuf),
    },
    UtilEntry {
        names: &["split"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(lines::split),
    },
    UtilEntry {
        names: &["expr"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(small::expr),
    },
    UtilEntry {
        names: &["factor"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(small::factor),
    },
    UtilEntry {
        names: &["envsubst"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(small::envsubst),
    },
    UtilEntry {
        names: &["truncate"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(small::truncate),
    },
    UtilEntry {
        names: &["sponge"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(small::sponge),
    },
    UtilEntry {
        names: &["yes"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(small::yes),
    },
    UtilEntry {
        names: &["uuidgen"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(small::uuidgen),
    },
    UtilEntry {
        names: &["tree"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(tree::run),
    },
    UtilEntry {
        names: &["dd"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(dd::dd),
    },
    UtilEntry {
        names: &["install"],
        mode: Mode::PathFallback,
        destructive: false,
        run: util_fn!(dd::install),
    },
    UtilEntry {
        names: &["find"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(find::run),
    },
    UtilEntry {
        names: &["xargs"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(xargs::run),
    },
    UtilEntry {
        names: &["timeout", "gtimeout"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(timeout::run),
    },
    UtilEntry {
        names: &["head"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(head::run),
    },
    UtilEntry {
        names: &["tail"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(tail::run),
    },
    UtilEntry {
        names: &["cat"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(cat::run),
    },
    UtilEntry {
        names: &["wc"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(wc::run),
    },
    UtilEntry {
        names: &["env"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(env::run),
    },
    UtilEntry {
        names: &["sleep"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(sleep::run),
    },
    UtilEntry {
        names: &["basename"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(basename::run),
    },
    UtilEntry {
        names: &["dirname"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(dirname::run),
    },
    UtilEntry {
        names: &["realpath"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(realpath::run),
    },
    UtilEntry {
        names: &["readlink"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(readlink::run),
    },
    UtilEntry {
        names: &["mkdir"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(mkdir::run),
    },
    UtilEntry {
        names: &["rmdir"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(rmdir::run),
    },
    UtilEntry {
        names: &["touch"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(touch::run),
    },
    UtilEntry {
        names: &["cut"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(cut::run),
    },
    UtilEntry {
        names: &["paste"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(paste::run),
    },
    UtilEntry {
        names: &["seq"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(seq::run),
    },
    UtilEntry {
        names: &["tee"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(tee::run),
    },
    UtilEntry {
        names: &["tr"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(tr::run),
    },
    UtilEntry {
        names: &["uniq"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(uniq::run),
    },
    UtilEntry {
        names: &["comm"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(comm::run),
    },
    UtilEntry {
        names: &["nl"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(nl::run),
    },
    UtilEntry {
        names: &["sort"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(sort::run),
    },
    UtilEntry {
        names: &["rm"],
        mode: Mode::Always,
        destructive: true,
        run: util_fn!(rm::run),
    },
    UtilEntry {
        names: &["mv"],
        mode: Mode::Always,
        destructive: true,
        run: util_fn!(mv::run),
    },
    UtilEntry {
        names: &["cp"],
        mode: Mode::Always,
        destructive: true,
        run: util_fn!(cp::run),
    },
    UtilEntry {
        names: &["ln"],
        mode: Mode::Always,
        destructive: true,
        run: util_fn!(ln::run),
    },
    UtilEntry {
        names: &["chmod"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(chmod::run),
    },
    UtilEntry {
        names: &["grep", "egrep", "fgrep"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(grep::run),
    },
    UtilEntry {
        names: &["fd"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(fd::run),
    },
    UtilEntry {
        names: &["rg"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(rg::run),
    },
    UtilEntry {
        names: &["base64"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(base64::run),
    },
    UtilEntry {
        names: &["cmp"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(cmp::run),
    },
    UtilEntry {
        names: &[
            "md5sum",
            "sha1sum",
            "sha224sum",
            "sha256sum",
            "sha384sum",
            "sha512sum",
        ],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(sum::run),
    },
    UtilEntry {
        names: &["md5"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(sum::run_md5),
    },
    UtilEntry {
        names: &["shasum"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(sum::run_shasum),
    },
    UtilEntry {
        names: &["diff"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(diff::run),
    },
    UtilEntry {
        names: &["date"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(date::run),
    },
    UtilEntry {
        names: &["which"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(which::run),
    },
    UtilEntry {
        names: &["printenv"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(env::run_printenv),
    },
    UtilEntry {
        names: &["sed"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(sed::run),
    },
    UtilEntry {
        names: &["ls"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(ls::run),
    },
    UtilEntry {
        names: &["stat"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(stat::run),
    },
    UtilEntry {
        names: &["du"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(du::run),
    },
    UtilEntry {
        names: &["mktemp"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(mktemp::run),
    },
];

/// What a host with no machine under it answers from what it says of itself
/// (nanocode's addition): not in [`REGISTRY`], so the psh program keeps the machine's.
pub static HOST_FACTS: &[UtilEntry] = &[
    UtilEntry {
        names: &["uname"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(small::uname),
    },
    UtilEntry {
        names: &["hostname"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(small::hostname),
    },
    UtilEntry {
        names: &["whoami"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(small::whoami),
    },
    UtilEntry {
        names: &["id"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(small::id),
    },
    UtilEntry {
        names: &["nproc"],
        mode: Mode::Always,
        destructive: false,
        run: util_fn!(small::nproc),
    },
];

/// The option specs, for the classifier check (D1082).
pub static SPECS: &[opts::Spec] = &[
    awk::SPEC,
    timeout::SPEC,
    xargs::SPEC,
    find::SPEC,
    jq::SPEC,
    head::SPEC,
    tail::SPEC,
    cat::SPEC,
    wc::SPEC,
    sleep::SPEC,
    env::SPEC,
    basename::SPEC,
    dirname::SPEC,
    realpath::SPEC,
    readlink::SPEC,
    mkdir::SPEC,
    rmdir::SPEC,
    touch::SPEC,
    cut::SPEC,
    paste::SPEC,
    seq::SPEC,
    tee::SPEC,
    tr::SPEC,
    uniq::SPEC,
    comm::SPEC,
    nl::SPEC,
    sort::SPEC,
    rm::SPEC,
    mv::SPEC,
    cp::SPEC,
    ln::SPEC,
    chmod::SPEC,
    grep::SPEC,
    fd::SPEC,
    grep::EGREP_SPEC,
    grep::FGREP_SPEC,
    base64::SPEC,
    cmp::SPEC,
    sum::SPEC,
    sum::MD5_SPEC,
    sum::SHASUM_SPEC,
    which::SPEC,
    date::SPEC,
    diff::SPEC,
    env::PRINTENV_SPEC,
    sed::SPEC,
    ls::SPEC,
    stat::SPEC,
    du::SPEC,
    mktemp::SPEC,
];

/// The policy for a shell: every utility on. `sandbox`: no real programs exist (wasm),
/// so `F` and `W` utilities always run.
pub fn policy(sandbox: bool) -> UtilPolicy {
    UtilPolicy {
        entries: REGISTRY,
        sandbox,
        ..UtilPolicy::default()
    }
}

/// The environment variable that forces utilities to run in-process in F and W mode
/// (D1192): a comma-separated list of names, or `all` for every such utility. The psh
/// binary and psh-spec read it and take it out of the environment.
pub const FORCE_VAR: &str = "PSH_UTILS_FORCE";

/// The environment variable that turns utilities off, so their names run the real
/// program: a comma-separated list of names, or `all` for every utility. The psh binary
/// reads it and takes it out of the environment.
pub const OFF_VAR: &str = "PSH_UTILS_OFF";

/// The names `PSH_UTILS_OFF`'s value turns off, when it is a list.
pub fn off_names(value: &[u8]) -> Vec<Vec<u8>> {
    value
        .split(|&b| b == b',')
        .filter(|n| !n.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

/// The names `PSH_UTILS_FORCE`'s value forces.
pub fn force_names(value: &[u8]) -> Vec<Vec<u8>> {
    if value == b"all" {
        return REGISTRY
            .iter()
            .filter(|e| e.mode != Mode::Always)
            .flat_map(|e| e.names.iter().map(|n| n.as_bytes().to_vec()))
            .collect();
    }
    value
        .split(|&b| b == b',')
        .filter(|n| !n.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

#[cfg(test)]
mod tests;

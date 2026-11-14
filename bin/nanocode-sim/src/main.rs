mod agents;
#[cfg(unix)]
mod alloc;
mod cells;
mod disk;
#[cfg(unix)]
mod egress;
mod executor;
mod faults;
mod host;
mod httpd;
mod httpserver;
mod log;
mod model;
mod mpp;
mod onelog;
mod rng;
mod scripted;
mod simulate;
mod socket;
mod store;
mod sync;
mod tidy;
mod turnlog;
mod turns;
mod whole;
mod workload;
mod world;
mod network;
mod server;
mod websocket;

use std::{collections::BTreeMap, ops::Range, process::ExitCode};

use crate::{
    agents::AgentStats,
    model::Stats,
    onelog::{LiveSeqs, Protections},
    simulate::{Options, run_seed},
    store::Mutation,
    turns::{Invariant, PolicyKind, TurnMutation, TurnOptions, TurnStats},
    httpd::WireMutation,
    httpserver::{HttpServerMutation, HttpServerStats},
    mpp::{MppMutation, MppStats},
    sync::{SyncMutation, SyncStats},
    websocket::{WebSocketMutation, WebSocketStats},
    whole::{StoreKind, WholeOptions, WholeStats, WireKind},
};

const DEFAULT_OPS: u64 = 1000;
const CAUGHT_SHOWN_MAX: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Store,
    Agents,
    Turns,
    Runtime,
    Websocket,
    HttpServer,
    Alloc,
    Egress,
    Mpp,
    Sync,
}

struct Args {
    seeds: Range<u64>,
    options: Options,
    twice: bool,
    scenario: Scenario,
    turns: TurnOptions,
    whole: WholeOptions,
    websocket: WebSocketMutation,
    http_server: HttpServerMutation,
    mpp: MppMutation,
    sync: SyncMutation,
    #[cfg(unix)]
    alloc: alloc::AllocMutation,
    #[cfg(unix)]
    egress: egress::EgressMutation,
}

type Caught = BTreeMap<Invariant, (u64, u64, (u64, u64))>;

fn usage() -> ExitCode {
    eprintln!(
        "usage: nanocode-sim --tidy | (--seed S | --seeds A..B) [--scenario store|agents|turns|runtime|websocket|http_server|alloc|egress|mpp|sync] [--ops N] [--twice] [--mutation {}] [--records content|slots] [--verbose]\n       turns: [--mutation {}] [--without {}]... [--live-seqs temporary|floor|gap|reuse] [--calm] [--policy reference|turnlog] [--row-limit BYTES] [--reopen-row-limit BYTES] [--long TURNS] [--snapshot-ratio K] [--snapshot-rows N] [--retain TURNS] [--state-bytes N] [--event-bytes N]\n       runtime: [--store memory|kv] [--calm] [--wire transport|http] [--mutation {}]\n       websocket: [--mutation {}]
       http_server: [--mutation {}]
       alloc: [--mutation overstate_size|forget_free|scribble]
       egress: [--mutation widen_allowlist|corrupt_tunnel_byte]
       mpp: [--mutation {}]
       sync: [--mutation {}]",
        Mutation::names(),
        TurnMutation::names(),
        onelog::PROTECTIONS.join("|"),
        WireMutation::NAMES,
        WebSocketMutation::names(),
        HttpServerMutation::names(),
        MppMutation::names(),
        SyncMutation::names()
    );
    ExitCode::from(2)
}

fn parse_seeds(value: &str) -> Option<Range<u64>> {
    if let Some((low, high)) = value.split_once("..") {
        return Some(low.parse().ok()?..high.parse().ok()?);
    }
    let seed: u64 = value.parse().ok()?;
    Some(seed..seed.checked_add(1)?)
}

fn parse_args(args: &[String]) -> Option<Args> {
    let mut seeds = None;
    let mut options = Options {
        ops: DEFAULT_OPS,
        mutation: Mutation::None,
        slots: false,
        verbose: false,
    };
    let mut twice = false;
    let mut scenario = Scenario::Store;
    let mut mutation = None;
    let mut turns = TurnOptions {
        calm: false,
        event_bytes: None,
        long: None,
        retain: None,
        snapshot_ratio: 2,
        snapshot_rows: None,
        state_bytes: None,
        mutation: TurnMutation::None,
        policy: PolicyKind::Reference,
        row_limit: None,
        reopen_row_limit: None,
        protections: Protections::ALL,
        verbose: false,
    };
    let mut whole = WholeOptions::default();
    let mut websocket = WebSocketMutation::None;
    let mut http_server = HttpServerMutation::None;
    let mut mpp = MppMutation::None;
    let mut sync = SyncMutation::None;
    #[cfg(unix)]
    let mut alloc = alloc::AllocMutation::None;
    #[cfg(unix)]
    let mut egress = egress::EgressMutation::None;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--twice" => twice = true,
            "--verbose" => options.verbose = true,
            "--calm" => turns.calm = true,
            "--store" => whole.store = Some(StoreKind::parse(rest.next()?)?),
            "--wire" => whole.wire = WireKind::parse(rest.next()?)?,
            "--policy" => turns.policy = PolicyKind::parse(rest.next()?)?,
            "--row-limit" => turns.row_limit = Some(rest.next()?.parse().ok()?),
            "--reopen-row-limit" => turns.reopen_row_limit = Some(rest.next()?.parse().ok()?),
            "--long" => turns.long = Some(rest.next()?.parse().ok()?),
            "--snapshot-ratio" => turns.snapshot_ratio = rest.next()?.parse().ok()?,
            "--snapshot-rows" => turns.snapshot_rows = Some(rest.next()?.parse().ok()?),
            "--retain" => turns.retain = Some(rest.next()?.parse().ok()?),
            "--state-bytes" => turns.state_bytes = Some(rest.next()?.parse().ok()?),
            "--event-bytes" => turns.event_bytes = Some(rest.next()?.parse().ok()?),
            "--without" => turns.protections = turns.protections.without(rest.next()?)?,
            "--live-seqs" => turns.protections.live_seqs = LiveSeqs::parse(rest.next()?)?,
            "--seed" | "--seeds" => seeds = Some(parse_seeds(rest.next()?)?),
            "--ops" => options.ops = rest.next()?.parse().ok()?,
            "--scenario" => {
                scenario = match rest.next()?.as_str() {
                    "store" => Scenario::Store,
                    "agents" => Scenario::Agents,
                    "turns" => Scenario::Turns,
                    "runtime" => Scenario::Runtime,
                    "websocket" => Scenario::Websocket,
                    "http_server" => Scenario::HttpServer,
                    "mpp" => Scenario::Mpp,
                    "sync" => Scenario::Sync,
                    #[cfg(unix)]
                    "alloc" => Scenario::Alloc,
                    #[cfg(unix)]
                    "egress" => Scenario::Egress,
                    _ => return None,
                };
            }
            "--mutation" => mutation = Some(rest.next()?.as_str()),
            "--records" => {
                options.slots = match rest.next()?.as_str() {
                    "content" => false,
                    "slots" => true,
                    _ => return None,
                };
            }
            _ => return None,
        }
    }
    match (scenario, mutation) {
        (_, None) => {}
        (Scenario::Store, Some(name)) => options.mutation = Mutation::parse(name)?,
        (Scenario::Turns, Some(name)) => turns.mutation = TurnMutation::parse(name)?,
        (Scenario::Runtime, Some(name)) => whole.mutation = WireMutation::parse(name)?,
        (Scenario::Websocket, Some(name)) => websocket = WebSocketMutation::parse(name)?,
        (Scenario::HttpServer, Some(name)) => http_server = HttpServerMutation::parse(name)?,
        (Scenario::Mpp, Some(name)) => mpp = MppMutation::parse(name)?,
        (Scenario::Sync, Some(name)) => sync = SyncMutation::parse(name)?,
        #[cfg(unix)]
        (Scenario::Alloc, Some(name)) => alloc = alloc::AllocMutation::parse(name)?,
        #[cfg(unix)]
        (Scenario::Egress, Some(name)) => egress = egress::EgressMutation::parse(name)?,
        #[cfg(not(unix))]
        (Scenario::Alloc, Some(_)) => return None,
        #[cfg(not(unix))]
        (Scenario::Egress, Some(_)) => return None,
        (Scenario::Agents, Some(_)) => return None,
    }
    turns.verbose = options.verbose;
    whole.verbose = options.verbose;
    whole.calm = turns.calm;
    Some(Args {
        seeds: seeds?,
        options,
        twice,
        scenario,
        turns,
        whole,
        websocket,
        http_server,
        mpp,
        sync,
        #[cfg(unix)]
        alloc,
        #[cfg(unix)]
        egress,
    })
}

fn replay(seed: u64, args: &Args) -> String {
    let options = args.options;
    if args.scenario == Scenario::Websocket {
        return websocket::replay(seed, args.websocket);
    }
    if args.scenario == Scenario::HttpServer {
        return httpserver::replay(seed, args.http_server);
    }
    if args.scenario == Scenario::Mpp {
        return mpp::replay(seed, args.mpp);
    }
    if args.scenario == Scenario::Sync {
        return sync::replay(seed, args.sync);
    }
    if args.scenario == Scenario::Agents {
        return format!("nanocode-sim --scenario agents --seed {seed}");
    }
    #[cfg(unix)]
    if args.scenario == Scenario::Alloc {
        return alloc::replay(seed, options.ops, args.alloc);
    }
    #[cfg(unix)]
    if args.scenario == Scenario::Egress {
        return egress::replay(seed, args.egress);
    }
    if args.scenario == Scenario::Runtime {
        let mut command = format!("nanocode-sim --scenario runtime --seed {seed}");
        if let Some(store) = args.whole.store {
            command.push_str(" --store ");
            command.push_str(store.name());
        }
        if args.whole.calm {
            command.push_str(" --calm");
        }
        if args.whole.wire != WireKind::Transport {
            command.push_str(" --wire ");
            command.push_str(args.whole.wire.name());
        }
        if args.whole.mutation != WireMutation::None {
            command.push_str(" --mutation ");
            command.push_str(args.whole.mutation.name());
        }
        return command;
    }
    if args.scenario == Scenario::Turns {
        let mut command = format!("nanocode-sim --scenario turns --seed {seed}");
        if args.turns.calm {
            command.push_str(" --calm");
        }
        if args.turns.policy != PolicyKind::Reference {
            command.push_str(" --policy ");
            command.push_str(args.turns.policy.name());
        }
        if let Some(limit) = args.turns.row_limit {
            command.push_str(&format!(" --row-limit {limit}"));
        }
        if let Some(limit) = args.turns.reopen_row_limit {
            command.push_str(&format!(" --reopen-row-limit {limit}"));
        }
        if let Some(turns) = args.turns.long {
            command.push_str(&format!(" --long {turns}"));
        }
        if args.turns.snapshot_ratio != 2 {
            command.push_str(&format!(" --snapshot-ratio {}", args.turns.snapshot_ratio));
        }
        if let Some(rows) = args.turns.snapshot_rows {
            command.push_str(&format!(" --snapshot-rows {rows}"));
        }
        if let Some(keep) = args.turns.retain {
            command.push_str(&format!(" --retain {keep}"));
        }
        if let Some(bytes) = args.turns.state_bytes {
            command.push_str(&format!(" --state-bytes {bytes}"));
        }
        if let Some(bytes) = args.turns.event_bytes {
            command.push_str(&format!(" --event-bytes {bytes}"));
        }
        for name in args.turns.protections.off() {
            command.push_str(" --without ");
            command.push_str(name);
        }
        if args.turns.protections.live_seqs != LiveSeqs::Temporary {
            command.push_str(" --live-seqs ");
            command.push_str(args.turns.protections.live_seqs.name());
        }
        if args.turns.mutation != TurnMutation::None {
            command.push_str(" --mutation ");
            command.push_str(args.turns.mutation.name());
        }
        return command;
    }
    let mut command = format!("nanocode-sim --seed {seed} --ops {}", options.ops);
    if options.mutation != Mutation::None {
        command.push_str(" --mutation ");
        command.push_str(options.mutation.name());
    }
    if options.slots {
        command.push_str(" --records slots");
    }
    command
}

fn judge(seed: u64, args: &Args, totals: &mut Stats) -> Option<String> {
    let first = run_seed(seed, args.options);
    totals.add(&first.stats);
    if args.options.verbose {
        println!("seed {seed} digest {:016x} {}", first.digest, first.shape);
    }
    if first.wrong.is_some() || !args.twice {
        return first
            .wrong
            .map(|message| format!("{} :: {message}", first.shape));
    }
    let quiet = Options {
        verbose: false,
        ..args.options
    };
    let second = run_seed(seed, quiet);
    (second.digest != first.digest || second.wrong.is_some()).then(|| {
        format!(
            "{} :: nondeterminism: trace digest {:016x} then {:016x}",
            first.shape, first.digest, second.digest
        )
    })
}

fn judge_agents(seed: u64, args: &Args, totals: &mut AgentStats) -> Option<String> {
    let first = agents::run_seed(seed);
    totals.add(&first.stats);
    if args.options.verbose {
        println!("seed {seed} digest {:016x} {}", first.digest, first.shape);
    }
    if first.wrong.is_some() || !args.twice {
        return first
            .wrong
            .map(|message| format!("{} :: {message}", first.shape));
    }
    let second = agents::run_seed(seed);
    (second.digest != first.digest || second.wrong.is_some()).then(|| {
        format!(
            "{} :: nondeterminism: trace digest {:016x} then {:016x}",
            first.shape, first.digest, second.digest
        )
    })
}

fn judge_whole(seed: u64, args: &Args, totals: &mut WholeStats) -> Option<String> {
    let first = whole::run_seed(seed, args.whole);
    totals.add(&first.stats);
    if args.options.verbose {
        println!("seed {seed} digest {:016x} {}", first.digest, first.shape);
    }
    if first.wrong.is_some() || !args.twice {
        return first
            .wrong
            .map(|message| format!("{} :: {message}", first.shape));
    }
    let quiet = WholeOptions {
        verbose: false,
        ..args.whole
    };
    let second = whole::run_seed(seed, quiet);
    (second.digest != first.digest || second.wrong.is_some()).then(|| {
        format!(
            "{} :: nondeterminism: trace digest {:016x} then {:016x}, {}",
            first.shape,
            first.digest,
            second.digest,
            whole::divergence(&first.lines, &second.lines)
        )
    })
}

fn judge_turns(
    seed: u64,
    args: &Args,
    totals: &mut TurnStats,
    caught: &mut Caught,
) -> Option<String> {
    let first = turns::run_seed(seed, args.turns);
    totals.add(&first.stats);
    if args.options.verbose {
        println!("seed {seed} digest {:016x} {}", first.digest, first.shape);
    }
    if let Some((invariant, message)) = &first.wrong {
        let entry = caught
            .entry(*invariant)
            .or_insert((0, seed, (first.lines, seed)));
        entry.0 += 1;
        if first.lines < entry.2.0 {
            entry.2 = (first.lines, seed);
        }
        return Some(format!(
            "{} :: {}: {message}",
            first.shape,
            invariant.name()
        ));
    }
    if !args.twice {
        return None;
    }
    let quiet = TurnOptions {
        verbose: false,
        ..args.turns
    };
    let second = turns::run_seed(seed, quiet);
    (second.digest != first.digest || second.wrong.is_some()).then(|| {
        format!(
            "{} :: nondeterminism: trace digest {:016x} then {:016x}",
            first.shape, first.digest, second.digest
        )
    })
}

fn report_turns(
    args: &Args,
    passed: u64,
    failed: u64,
    totals: &TurnStats,
    caught: &Caught,
) -> ExitCode {
    for (invariant, (count, first, (lines, shortest))) in caught {
        println!(
            "INVARIANT {} failed={count} first_seed={first} shortest_seed={shortest} trace_lines={lines}",
            invariant.name()
        );
    }
    println!(
        "RESULT seeds={}..{} scenario=turns policy={} calm={} protections_off={:?} live_seqs={} mutation={} passed={passed} failed={failed} twice={} {} stats={totals:?}",
        args.seeds.start,
        args.seeds.end,
        args.turns.policy.name(),
        args.turns.calm,
        args.turns.protections.off(),
        args.turns.protections.live_seqs.name(),
        args.turns.mutation.name(),
        args.twice,
        totals.summary()
    );
    let mutation = args.turns.mutation;
    if mutation != TurnMutation::None {
        if failed > 0 {
            println!("MUTATION {} caught by {failed} seeds", mutation.name());
            return ExitCode::SUCCESS;
        }
        println!("MUTATION {} NOT caught", mutation.name());
        return ExitCode::FAILURE;
    }
    if args.turns.protections != Protections::ALL || failed == 0 {
        return ExitCode::SUCCESS;
    }
    ExitCode::FAILURE
}

#[cfg(unix)]
fn alloc_experiment(args: &Args) -> bool {
    (args.scenario == Scenario::Alloc && args.alloc != alloc::AllocMutation::None)
        || (args.scenario == Scenario::Egress && args.egress != egress::EgressMutation::None)
}

#[cfg(not(unix))]
const fn alloc_experiment(_args: &Args) -> bool {
    false
}

fn tidy() -> ExitCode {
    match tidy::check(&tidy::workspace_root()) {
        Ok(problems) if problems.is_empty() => {
            println!("TIDY determinism: clean");
            ExitCode::SUCCESS
        }
        Ok(problems) => {
            for problem in problems {
                println!("TIDY {problem}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("TIDY could not read the sources: {error}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--tidy"] {
        return tidy();
    }
    log::install();
    if let Err(error) = host::install() {
        eprintln!("could not install the simulated platform host: {error}");
        return ExitCode::FAILURE;
    }
    let Some(args) = parse_args(&args) else {
        return usage();
    };
    let mutation = args.options.mutation;
    let experiment = (args.scenario == Scenario::Turns
        && (args.turns.mutation != TurnMutation::None
            || args.turns.protections != Protections::ALL))
        || (args.scenario == Scenario::Runtime && args.whole.mutation != WireMutation::None)
        || (args.scenario == Scenario::Websocket && args.websocket != WebSocketMutation::None)
        || (args.scenario == Scenario::HttpServer && args.http_server != HttpServerMutation::None)
        || (args.scenario == Scenario::Mpp && args.mpp != MppMutation::None)
        || (args.scenario == Scenario::Sync && args.sync != SyncMutation::None)
        || alloc_experiment(&args);
    let (mut passed, mut failed) = (0_u64, 0_u64);
    let mut totals = Stats::default();
    let mut agent_totals = AgentStats::default();
    let mut turn_totals = TurnStats::default();
    let mut whole_totals = WholeStats::default();
    let mut websocket_totals = WebSocketStats::default();
    let mut http_server_totals = HttpServerStats::default();
    let mut mpp_totals = MppStats::default();
    let mut sync_totals = SyncStats::default();
    #[cfg(unix)]
    let mut alloc_totals = alloc::AllocStats::default();
    #[cfg(unix)]
    let mut egress_totals = egress::EgressStats::default();
    let mut caught = Caught::new();
    for seed in args.seeds.clone() {
        let verdict = match args.scenario {
            Scenario::Store => judge(seed, &args, &mut totals),
            Scenario::Agents => judge_agents(seed, &args, &mut agent_totals),
            Scenario::Turns => judge_turns(seed, &args, &mut turn_totals, &mut caught),
            Scenario::Runtime => judge_whole(seed, &args, &mut whole_totals),
            Scenario::Websocket => websocket::judge(
                seed,
                args.websocket,
                args.twice,
                args.options.verbose,
                &mut websocket_totals,
            ),
            Scenario::HttpServer => httpserver::judge(
                seed,
                args.http_server,
                args.twice,
                args.options.verbose,
                &mut http_server_totals,
            ),
            Scenario::Mpp => mpp::judge_seed(
                seed,
                args.mpp,
                args.twice,
                args.options.verbose,
                &mut mpp_totals,
            ),
            Scenario::Sync => sync::judge_seed(
                seed,
                args.sync,
                args.twice,
                args.options.verbose,
                &mut sync_totals,
            ),
            #[cfg(unix)]
            Scenario::Alloc => alloc::judge(
                seed,
                args.options.ops,
                args.alloc,
                args.twice,
                args.options.verbose,
                &mut alloc_totals,
            ),
            #[cfg(not(unix))]
            Scenario::Alloc => unreachable!("the alloc scenario parses only on unix"),
            #[cfg(unix)]
            Scenario::Egress => egress::judge_seed(
                seed,
                args.egress,
                args.twice,
                args.options.verbose,
                &mut egress_totals,
            ),
            #[cfg(not(unix))]
            Scenario::Egress => unreachable!("the egress scenario parses only on unix"),
        };
        let Some(message) = verdict else {
            passed += 1;
            continue;
        };
        failed += 1;
        let replay = replay(seed, &args);
        if mutation == Mutation::None && !experiment {
            println!("FAIL seed={seed} Wrong {message} :: replay: {replay}");
        } else if failed <= u64::try_from(CAUGHT_SHOWN_MAX).expect("a small count fits in u64") {
            println!("CAUGHT seed={seed} {message} :: replay: {replay}");
        }
    }
    if args.scenario == Scenario::Websocket {
        return websocket::report(
            &args.seeds,
            passed,
            failed,
            args.twice,
            args.websocket,
            &websocket_totals,
        );
    }
    if args.scenario == Scenario::Sync {
        return sync::report(&args.seeds, passed, failed, args.twice, args.sync, &sync_totals);
    }
    if args.scenario == Scenario::Mpp {
        return mpp::report(&args.seeds, passed, failed, args.twice, args.mpp, &mpp_totals);
    }
    if args.scenario == Scenario::HttpServer {
        return httpserver::report(
            &args.seeds,
            passed,
            failed,
            args.twice,
            args.http_server,
            &http_server_totals,
        );
    }
    #[cfg(unix)]
    if args.scenario == Scenario::Alloc {
        return alloc::report(&args.seeds, passed, failed, args.twice, args.alloc, &alloc_totals);
    }
    #[cfg(unix)]
    if args.scenario == Scenario::Egress {
        return egress::report(&args.seeds, passed, failed, args.twice, args.egress, &egress_totals);
    }
    if args.scenario == Scenario::Turns {
        return report_turns(&args, passed, failed, &turn_totals, &caught);
    }
    if args.scenario == Scenario::Runtime {
        println!(
            "RESULT seeds={}..{} scenario=runtime wire={} mutation={} passed={passed} failed={failed} twice={} stats={whole_totals:?}",
            args.seeds.start,
            args.seeds.end,
            args.whole.wire.name(),
            args.whole.mutation.name(),
            args.twice
        );
        if args.whole.mutation != WireMutation::None {
            if failed > 0 {
                println!("MUTATION {} caught by {failed} seeds", args.whole.mutation.name());
                return ExitCode::SUCCESS;
            }
            println!("MUTATION {} NOT caught", args.whole.mutation.name());
            return ExitCode::FAILURE;
        }
        return if failed == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if args.scenario == Scenario::Agents {
        println!(
            "RESULT seeds={}..{} scenario=agents passed={passed} failed={failed} twice={} stats={agent_totals:?}",
            args.seeds.start, args.seeds.end, args.twice
        );
        return if failed == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    println!(
        "RESULT seeds={}..{} mutation={} passed={passed} failed={failed} twice={} stats={totals:?}",
        args.seeds.start,
        args.seeds.end,
        mutation.name(),
        args.twice
    );
    match mutation {
        Mutation::None if failed == 0 => ExitCode::SUCCESS,
        Mutation::None => ExitCode::FAILURE,
        _ if failed > 0 => {
            println!("MUTATION {} caught by {failed} seeds", mutation.name());
            ExitCode::SUCCESS
        }
        _ => {
            println!("MUTATION {} NOT caught", mutation.name());
            ExitCode::FAILURE
        }
    }
}

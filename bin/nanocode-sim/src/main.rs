mod agents;
mod disk;
mod executor;
mod faults;
mod host;
mod model;
mod onelog;
mod rng;
mod scripted;
mod simulate;
mod store;
mod tidy;
mod turnlog;
mod turns;
mod workload;

use std::{collections::BTreeMap, ops::Range, process::ExitCode};

use crate::{
    agents::AgentStats,
    model::Stats,
    onelog::{LiveSeqs, Protections},
    simulate::{Options, run_seed},
    store::Mutation,
    turns::{Invariant, PolicyKind, TurnMutation, TurnOptions, TurnStats},
};

const DEFAULT_OPS: u64 = 1000;
const CAUGHT_SHOWN_MAX: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Store,
    Agents,
    Turns,
}

struct Args {
    seeds: Range<u64>,
    options: Options,
    twice: bool,
    scenario: Scenario,
    turns: TurnOptions,
}

type Caught = BTreeMap<Invariant, (u64, u64, (u64, u64))>;

fn usage() -> ExitCode {
    eprintln!(
        "usage: nanocode-sim --tidy | (--seed S | --seeds A..B) [--scenario store|agents|turns] [--ops N] [--twice] [--mutation {}] [--records content|slots] [--verbose]\n       turns: [--mutation {}] [--without {}]... [--live-seqs temporary|floor|gap|reuse] [--calm] [--policy reference|turnlog] [--row-limit BYTES]",
        Mutation::names(),
        TurnMutation::names(),
        onelog::PROTECTIONS.join("|")
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
        mutation: TurnMutation::None,
        policy: PolicyKind::Reference,
        row_limit: None,
        protections: Protections::ALL,
        verbose: false,
    };
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--twice" => twice = true,
            "--verbose" => options.verbose = true,
            "--calm" => turns.calm = true,
            "--policy" => turns.policy = PolicyKind::parse(rest.next()?)?,
            "--row-limit" => turns.row_limit = Some(rest.next()?.parse().ok()?),
            "--without" => turns.protections = turns.protections.without(rest.next()?)?,
            "--live-seqs" => turns.protections.live_seqs = LiveSeqs::parse(rest.next()?)?,
            "--seed" | "--seeds" => seeds = Some(parse_seeds(rest.next()?)?),
            "--ops" => options.ops = rest.next()?.parse().ok()?,
            "--scenario" => {
                scenario = match rest.next()?.as_str() {
                    "store" => Scenario::Store,
                    "agents" => Scenario::Agents,
                    "turns" => Scenario::Turns,
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
        (Scenario::Agents, Some(_)) => return None,
    }
    turns.verbose = options.verbose;
    Some(Args {
        seeds: seeds?,
        options,
        twice,
        scenario,
        turns,
    })
}

fn replay(seed: u64, args: &Args) -> String {
    let options = args.options;
    if args.scenario == Scenario::Agents {
        return format!("nanocode-sim --scenario agents --seed {seed}");
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
    if let Err(error) = host::install() {
        eprintln!("could not install the simulated platform host: {error}");
        return ExitCode::FAILURE;
    }
    let Some(args) = parse_args(&args) else {
        return usage();
    };
    let mutation = args.options.mutation;
    let experiment = args.scenario == Scenario::Turns
        && (args.turns.mutation != TurnMutation::None
            || args.turns.protections != Protections::ALL);
    let (mut passed, mut failed) = (0_u64, 0_u64);
    let mut totals = Stats::default();
    let mut agent_totals = AgentStats::default();
    let mut turn_totals = TurnStats::default();
    let mut caught = Caught::new();
    for seed in args.seeds.clone() {
        let verdict = match args.scenario {
            Scenario::Store => judge(seed, &args, &mut totals),
            Scenario::Agents => judge_agents(seed, &args, &mut agent_totals),
            Scenario::Turns => judge_turns(seed, &args, &mut turn_totals, &mut caught),
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
    if args.scenario == Scenario::Turns {
        return report_turns(&args, passed, failed, &turn_totals, &caught);
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

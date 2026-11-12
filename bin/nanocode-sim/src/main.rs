mod executor;
mod faults;
mod model;
mod rng;
mod simulate;
mod store;
mod workload;

use std::{ops::Range, process::ExitCode};

use crate::{
    model::Stats,
    simulate::{Options, run_seed},
    store::Mutation,
};

const DEFAULT_OPS: u64 = 1000;
const CAUGHT_SHOWN_MAX: usize = 3;

struct Args {
    seeds: Range<u64>,
    options: Options,
    twice: bool,
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: nanocode-sim (--seed S | --seeds A..B) [--ops N] [--twice] [--mutation {}] [--records content|slots] [--verbose]",
        Mutation::names()
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
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--twice" => twice = true,
            "--verbose" => options.verbose = true,
            "--seed" | "--seeds" => seeds = Some(parse_seeds(rest.next()?)?),
            "--ops" => options.ops = rest.next()?.parse().ok()?,
            "--mutation" => options.mutation = Mutation::parse(rest.next()?)?,
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
    Some(Args {
        seeds: seeds?,
        options,
        twice,
    })
}

fn replay(seed: u64, options: Options) -> String {
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(args) = parse_args(&args) else {
        return usage();
    };
    let mutation = args.options.mutation;
    let (mut passed, mut failed) = (0_u64, 0_u64);
    let mut totals = Stats::default();
    for seed in args.seeds.clone() {
        let Some(message) = judge(seed, &args, &mut totals) else {
            passed += 1;
            continue;
        };
        failed += 1;
        let replay = replay(seed, args.options);
        if mutation == Mutation::None {
            println!("FAIL seed={seed} Wrong {message} :: replay: {replay}");
        } else if failed <= u64::try_from(CAUGHT_SHOWN_MAX).expect("a small count fits in u64") {
            println!("CAUGHT seed={seed} {message} :: replay: {replay}");
        }
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

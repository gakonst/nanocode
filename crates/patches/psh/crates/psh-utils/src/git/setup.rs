//! What needs no repository, or makes one: git init, and git config where it reads
//! and writes configuration files.

use std::io::Write;

use super::repo::{Config, global_config};
use super::store::{Fs, join};
use super::write::{Edit, Mode, Op, Plan, edit_config};
use super::{Error, Io, Res, config_key, option};

const MASTER_HINT: &str = "hint: Using 'master' as the name for the initial branch. This default branch name\nhint: is subject to change. To configure the initial branch name to use in all\nhint: of your new repositories, which will suppress this warning, call:\nhint:\nhint: \tgit config --global init.defaultBranch <name>\nhint:\nhint: Names commonly chosen instead of 'master' are 'main', 'trunk' and\nhint: 'development'. The just-created branch can be renamed via this command:\nhint:\nhint: \tgit branch -m <name>\nhint:\nhint: Disable this message with \"git config set advice.defaultBranchName false\"\n";

const EXCLUDE: &str = "# git ls-files --others --exclude-from=.git/info/exclude\n# Lines that start with '#' are comments.\n# For a project mostly in C, the following would be a good set of\n# exclude patterns (uncomment them if you want to use them):\n# *.[oa]\n# *~\n";

/// The commands that run before a repository is looked for: init, and config --global.
pub fn outside(
    fs: &Fs<'_>,
    cwd: &[u8],
    env: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
    command: &str,
    args: &[String],
    io: &mut Io<'_>,
    plan: &mut Plan,
) -> Res<Option<i32>> {
    match command {
        "init" => init(fs, cwd, env, args, io, plan).map(Some),
        "config" if args.iter().any(|a| a == "--global") => {
            let mut push = |op: Op| plan.ops.push(op);
            config(fs, env, None, args, io, &mut push).map(Some)
        }
        _ => Ok(None),
    }
}

fn init(
    fs: &Fs<'_>,
    cwd: &[u8],
    env: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
    args: &[String],
    io: &mut Io<'_>,
    plan: &mut Plan,
) -> Res<i32> {
    let (mut quiet, mut bare) = (false, false);
    let mut branch: Option<String> = None;
    let mut dir: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-q" | "--quiet" => quiet = true,
            "--bare" => bare = true,
            "-b" | "--initial-branch" => {
                i += 1;
                branch = args.get(i).cloned();
            }
            _ if arg.starts_with("--initial-branch=") => branch = Some(arg[17..].to_string()),
            _ if arg.starts_with('-') => return Err(option("init", arg)),
            _ if dir.is_none() => dir = Some(arg.to_string()),
            _ => return Err(Error::Usage("usage: git init [-q | --quiet] [--bare] [-b <branch-name> | --initial-branch=<branch-name>] [<directory>]".to_string())),
        }
        i += 1;
    }
    let top = match &dir {
        Some(dir) if dir.starts_with('/') => super::repo::normalize(dir.as_bytes()),
        Some(dir) => super::repo::normalize(&join(cwd, dir.as_bytes())),
        None => cwd.to_vec(),
    };
    let git_dir = if bare {
        top.clone()
    } else {
        join(&top, b".git")
    };
    // The directory as the file system names it, where it is there to ask.
    let mut shown = fs.physical(&git_dir);
    if shown == git_dir && fs.stat_opt(&top, true)?.is_none() {
        let parent = top
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(&top[..], |cut| &top[..cut.max(1)]);
        let real = fs.physical(parent);
        if real != parent {
            shown = join(
                &real,
                &git_dir[parent.len()..]
                    .iter()
                    .skip(1)
                    .copied()
                    .collect::<Vec<u8>>(),
            );
        }
    }
    let shown = String::from_utf8_lossy(&shown).into_owned();
    if fs.stat_opt(&join(&git_dir, b"HEAD"), false)?.is_some() {
        for dir in ["refs/heads", "refs/tags", "objects/info", "objects/pack"] {
            plan.ops.push(Op::Mkdir(join(&git_dir, dir.as_bytes())));
        }
        if !quiet {
            writeln!(io.out, "Reinitialized existing Git repository in {shown}/")?;
        }
        return Ok(0);
    }
    let mut global = Config::default();
    for path in global_config(env) {
        if let Some(text) = fs.read_opt(&path)? {
            global.parse(&text);
        }
    }
    let branch = match branch.or_else(|| global.get("init.defaultbranch").map(str::to_string)) {
        Some(branch) => branch,
        None => {
            if !quiet && global.bool("advice.defaultbranchname") != Some(false) {
                io.warn(MASTER_HINT);
            }
            "master".to_string()
        }
    };
    if !super::write::valid_ref(&format!("refs/heads/{branch}")) {
        return Err(Error::Fatal(format!(
            "invalid initial branch name: '{branch}'"
        )));
    }
    for dir in [
        "hooks",
        "info",
        "objects/info",
        "objects/pack",
        "refs/heads",
        "refs/tags",
    ] {
        plan.ops.push(Op::Mkdir(join(&git_dir, dir.as_bytes())));
    }
    let mut file = |name: &str, text: String| {
        plan.ops.push(Op::Write {
            path: join(&git_dir, name.as_bytes()),
            data: text.into_bytes(),
            mode: Mode::Plain,
        });
    };
    file("HEAD", format!("ref: refs/heads/{branch}\n"));
    file(
        "config",
        if bare {
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = true\n".to_string()
        } else {
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n\tlogallrefupdates = true\n".to_string()
        },
    );
    file(
        "description",
        "Unnamed repository; edit this file 'description' to name the repository.\n".to_string(),
    );
    file("info/exclude", EXCLUDE.to_string());
    if !quiet {
        writeln!(io.out, "Initialized empty Git repository in {shown}/")?;
    }
    Ok(0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Get,
    GetAll,
    List,
    Set,
    Add,
    Unset,
    RemoveSection,
    RenameSection,
}

/// git config. local is the repository's own file and what all its files say
/// together, when the command runs in one.
pub fn config(
    fs: &Fs<'_>,
    env: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
    local: Option<(&[u8], &Config)>,
    args: &[String],
    io: &mut Io<'_>,
    push: &mut dyn FnMut(Op),
) -> Res<i32> {
    let mut action = None;
    let (mut global, mut only_local) = (false, false);
    let mut words: Vec<&str> = Vec::new();
    for arg in args {
        let named = match arg.as_str() {
            "--global" => {
                global = true;
                continue;
            }
            "--local" => {
                only_local = true;
                continue;
            }
            "--get" => Action::Get,
            "--get-all" => Action::GetAll,
            "-l" | "--list" => Action::List,
            "--add" => Action::Add,
            "--replace-all" => Action::Set,
            "--unset" | "--unset-all" => Action::Unset,
            "--remove-section" => Action::RemoveSection,
            "--rename-section" => Action::RenameSection,
            "get" | "set" | "unset" | "list" | "remove-section" | "rename-section"
                if words.is_empty() && action.is_none() =>
            {
                match arg.as_str() {
                    "get" => Action::Get,
                    "set" => Action::Set,
                    "unset" => Action::Unset,
                    "list" => Action::List,
                    "remove-section" => Action::RemoveSection,
                    _ => Action::RenameSection,
                }
            }
            a if a.starts_with('-') => return Err(option("config", a)),
            a => {
                words.push(a);
                continue;
            }
        };
        action = Some(named);
    }
    let action = match (action, words.len()) {
        (Some(action), _) => action,
        (None, 1) => Action::Get,
        (None, 2) => Action::Set,
        (None, _) => {
            return Err(Error::Usage(
                "error: wrong number of arguments, should be from 1 to 2".to_string(),
            ));
        }
    };
    // The file a scope names: to write, and to read when the scope was given.
    let path = if global {
        let paths = global_config(env);
        let mut found = None;
        for path in paths.iter().rev() {
            if fs.stat_opt(path, true)?.is_some() {
                found = Some(path.clone());
                break;
            }
        }
        match found.or_else(|| paths.last().cloned()) {
            Some(path) => path,
            None => return Err(Error::Fatal("$HOME not set".to_string())),
        }
    } else {
        match local {
            Some((path, _)) => path.to_vec(),
            None => return Err(Error::Fatal("not in a git directory".to_string())),
        }
    };
    let scoped = global || only_local;
    let mut own = Config::default();
    let text = fs.read_opt(&path)?;
    if let Some(text) = &text {
        own.parse(text);
    }
    let read: &Config = match (scoped, local) {
        (false, Some((_, all))) => all,
        _ => &own,
    };
    let key_of = |word: &str| -> Res<String> {
        if !word.contains('.') {
            return Err(Error::Says(
                format!("error: key does not contain a section: {word}\n"),
                if matches!(action, Action::Get | Action::GetAll) {
                    1
                } else {
                    2
                },
            ));
        }
        Ok(config_key(word))
    };
    let text = String::from_utf8_lossy(text.as_deref().unwrap_or_default()).into_owned();
    let mut save = |text: String| {
        push(Op::Write {
            path: path.clone(),
            data: text.into_bytes(),
            mode: Mode::Plain,
        });
    };
    match (action, words.as_slice()) {
        (Action::List, []) => {
            for (key, value) in &read.entries {
                writeln!(io.out, "{key}={value}")?;
            }
            Ok(0)
        }
        (Action::Get, [key]) => match read.get(&key_of(key)?) {
            Some(value) => {
                writeln!(io.out, "{value}")?;
                Ok(0)
            }
            None => Ok(1),
        },
        (Action::GetAll, [key]) => {
            let key = key_of(key)?;
            let mut status = 1;
            for (k, value) in &read.entries {
                if *k == key {
                    writeln!(io.out, "{value}")?;
                    status = 0;
                }
            }
            Ok(status)
        }
        (Action::Set, [key, value]) | (Action::Add, [key, value]) => {
            let edit = if action == Action::Add {
                Edit::Add(value)
            } else {
                Edit::Set(value)
            };
            if let Some(text) = edit_config(&text, &key_of(key)?, edit) {
                save(text);
            }
            Ok(0)
        }
        (Action::Unset, [key]) => match edit_config(&text, &key_of(key)?, Edit::Unset) {
            Some(text) => {
                save(text);
                Ok(0)
            }
            None => Ok(5),
        },
        (Action::RemoveSection, [section]) => {
            match edit_config(
                &text,
                config_key(&format!("{section}.x")).trim_end_matches(".x"),
                Edit::RemoveSection,
            ) {
                Some(text) => {
                    save(text);
                    Ok(0)
                }
                None => Err(Error::Fatal(format!("no such section: {section}"))),
            }
        }
        (Action::RenameSection, [from, to]) => {
            let key = config_key(&format!("{from}.x"));
            match edit_config(&text, key.trim_end_matches(".x"), Edit::RenameSection(to)) {
                Some(text) => {
                    save(text);
                    Ok(0)
                }
                None => Err(Error::Fatal(format!("no such section: {from}"))),
            }
        }
        _ => Err(Error::Usage(
            "error: wrong number of arguments, should be from 1 to 2".to_string(),
        )),
    }
}

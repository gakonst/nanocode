// Adapted from fd (https://github.com/sharkdp/fd, commit 193d0df), src/exec/mod.rs,
// src/exec/command.rs and src/exec/job.rs, with the psh patches marked `psh:` (D1138):
// Copyright (c) 2017-present The fd developers. Licensed under the MIT License (see NOTICE).

// psh: commands run through the command runner (`UtilCtx::spawn`: utilities, standalone
// builtins, programs), one after another in result order, from the async front end
// once the walk has collected the results (a blocking body cannot spawn). fd runs `-x`
// commands on several threads in no particular order; with psh the order is the
// results' (what fd does with `--threads=1`). There is no argmax: a batch is cut at
// `BATCH_LIMIT` bytes of arguments, as psh's find does.

use psh_engine::util::{STANDALONE_BUILTINS, SpawnEnv, UtilCtx};

use super::fmt::{FormatTemplate, Token};

/// The size of a batch's command line.
const BATCH_LIMIT: usize = 128 * 1024;

/// Execution mode of the command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Command is executed for each search result
    OneByOne,
    /// Command is run for a batch of results at once
    Batch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandSet {
    mode: ExecutionMode,
    commands: Vec<CommandTemplate>,
}

/// How the commands run (psh: what fd's `Config` gives `execute`).
pub struct ExecOptions {
    pub path_separator: Option<String>,
    pub null_separator: bool,
    /// fd buffers command output with more than one thread, and then gives the
    /// commands no stdin (`Command::output`).
    pub buffer_output: bool,
    pub batch_size: usize,
    /// The commands' working directory (`--base-directory`), an absolute shell path.
    pub cwd: Option<Vec<u8>>,
}

impl CommandSet {
    pub fn new(input: Vec<Vec<Vec<u8>>>) -> Result<Self, String> {
        Ok(Self {
            mode: ExecutionMode::OneByOne,
            commands: input
                .into_iter()
                .map(|args| CommandTemplate::new(args, ExecutionMode::OneByOne))
                .collect::<Result<_, String>>()?,
        })
    }

    pub fn new_batch(input: Vec<Vec<Vec<u8>>>) -> Result<Self, String> {
        Ok(Self {
            mode: ExecutionMode::Batch,
            commands: input
                .into_iter()
                .map(|args| {
                    let cmd = CommandTemplate::new(args, ExecutionMode::Batch)?;
                    if cmd.number_of_tokens() > 1 {
                        return Err("Only one placeholder allowed for batch commands".to_string());
                    }
                    Ok(cmd)
                })
                .collect::<Result<Vec<_>, String>>()?,
        })
    }

    pub fn in_batch_mode(&self) -> bool {
        self.mode == ExecutionMode::Batch
    }

    /// Runs the commands for each result (`-x`): fd's `job` and `execute_commands`.
    pub async fn execute(&self, ctx: &mut UtilCtx<'_>, paths: &[Vec<u8>], o: &ExecOptions) -> i32 {
        let mut ret = 0;
        for path in paths {
            for c in &self.commands {
                let argv = c.generate(path, o.path_separator.as_deref());
                // Then wait for the command to exit, if it was spawned.
                let code = run(ctx, argv, o, o.buffer_output).await;
                if code != 0 {
                    ret = 1;
                    break;
                }
            }
            if o.null_separator {
                // If null_separator is enabled, then we should write a \0 at the end
                // of the output for this entry
                let _ = ctx.write(b"\0").await;
                let _ = ctx.flush().await;
            }
        }
        // Returns error in case of any error.
        ret
    }

    /// Runs each command once with all results (`-X`), in batches.
    pub async fn execute_batch(
        &self,
        ctx: &mut UtilCtx<'_>,
        paths: &[Vec<u8>],
        o: &ExecOptions,
    ) -> i32 {
        let mut builders: Vec<CommandBuilder> = self
            .commands
            .iter()
            .map(|c| CommandBuilder::new(c, o.batch_size))
            .collect();
        for path in paths {
            for builder in &mut builders {
                if let Err(code) = builder.push(ctx, path, o).await {
                    return code;
                }
            }
        }
        for builder in &mut builders {
            if let Err(code) = builder.finish(ctx, o).await {
                return code;
            }
        }
        i32::from(builders.iter().any(|b| b.exit_code != 0))
    }
}

/// Runs one command line: its status, or 1 with fd's message when there is no such
/// command (fd's `handle_cmd_error`).
async fn run(ctx: &mut UtilCtx<'_>, argv: Vec<Vec<u8>>, o: &ExecOptions, buffered: bool) -> i32 {
    if !exists(ctx, &argv[0], o.cwd.as_deref()).await {
        let name = String::from_utf8_lossy(&argv[0]).into_owned();
        let msg = format!("Command not found: {name:?}");
        let line = format!("[fd error]: {}\n", super::sanitize_error(&msg));
        ctx.error_raw(line.as_bytes()).await;
        return 1;
    }
    let env = SpawnEnv {
        cwd: o.cwd.clone(),
        stdin: buffered.then(|| b"/dev/null".to_vec()),
        ..SpawnEnv::default()
    };
    let mut child = ctx.spawn(argv, env).await;
    child.wait().await
}

/// Whether a command name names something to run: a path, a standalone builtin, a
/// utility or a program on `PATH`.
async fn exists(ctx: &mut UtilCtx<'_>, name: &[u8], cwd: Option<&[u8]>) -> bool {
    if name.is_empty() {
        return false;
    }
    if name.contains(&b'/') || (cfg!(windows) && name.contains(&b'\\')) {
        let p = match cwd {
            Some(c) if !crate::search::walk::is_absolute(name) => {
                crate::search::walk::join(c, name)
            }
            _ => name.to_vec(),
        };
        return ctx.stat(&p, true).await.is_ok();
    }
    if STANDALONE_BUILTINS.iter().any(|b| b.as_bytes() == name) || ctx.is_utility(name).await {
        return true;
    }
    let path = ctx.var(b"PATH").unwrap_or_default().to_vec();
    !ctx.search_path(name, &path).await.is_empty()
}

/// Represents a multi-exec command as it is built.
#[derive(Debug)]
struct CommandBuilder {
    pre_args: Vec<Vec<u8>>,
    path_arg: FormatTemplate,
    post_args: Vec<Vec<u8>>,
    args: Vec<Vec<u8>>,
    size: usize,
    count: usize,
    limit: usize,
    exit_code: i32,
}

impl CommandBuilder {
    fn new(template: &CommandTemplate, limit: usize) -> Self {
        let mut pre_args = vec![];
        let mut path_arg = None;
        let mut post_args = vec![];

        for arg in &template.args {
            if arg.has_tokens() {
                path_arg = Some(arg.clone());
            } else if path_arg.is_none() {
                pre_args.push(arg.generate(b"", None));
            } else {
                post_args.push(arg.generate(b"", None));
            }
        }

        let mut b = Self {
            pre_args,
            // A batch template has one placeholder (`new` appends one if none).
            path_arg: path_arg.unwrap_or(FormatTemplate::Tokens(vec![Token::Placeholder])),
            post_args,
            args: Vec::new(),
            size: 0,
            count: 0,
            limit,
            exit_code: 0,
        };
        b.reset();
        b
    }

    fn reset(&mut self) {
        self.args = self.pre_args.clone();
        self.size = self.args.iter().map(|a| a.len() + 1).sum();
        self.count = 0;
    }

    async fn push(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        path: &[u8],
        o: &ExecOptions,
    ) -> Result<(), i32> {
        if self.limit > 0 && self.count >= self.limit {
            self.finish(ctx, o).await?;
        }

        let arg = self.path_arg.generate(path, o.path_separator.as_deref());
        let post: usize = self.post_args.iter().map(|a| a.len() + 1).sum();
        if self.count > 0 && self.size + arg.len() + 1 + post > BATCH_LIMIT {
            self.finish(ctx, o).await?;
        }

        self.size += arg.len() + 1;
        self.args.push(arg);
        self.count += 1;
        Ok(())
    }

    async fn finish(&mut self, ctx: &mut UtilCtx<'_>, o: &ExecOptions) -> Result<(), i32> {
        if self.count > 0 {
            let mut argv = std::mem::take(&mut self.args);
            argv.extend(self.post_args.iter().cloned());
            if !exists(ctx, &argv[0], o.cwd.as_deref()).await {
                let name = String::from_utf8_lossy(&argv[0]).into_owned();
                let msg = format!("Command not found: {name:?}");
                let line = format!("[fd error]: {}\n", super::sanitize_error(&msg));
                ctx.error_raw(line.as_bytes()).await;
                return Err(1);
            }
            let env = SpawnEnv {
                cwd: o.cwd.clone(),
                ..SpawnEnv::default()
            };
            let mut child = ctx.spawn(argv, env).await;
            if child.wait().await != 0 {
                self.exit_code = 1;
            }
            self.reset();
        }
        Ok(())
    }
}

/// Represents a template that is utilized to generate command strings.
#[derive(Debug, Clone, PartialEq)]
struct CommandTemplate {
    args: Vec<FormatTemplate>,
}

impl CommandTemplate {
    fn new(input: Vec<Vec<u8>>, mode: ExecutionMode) -> Result<Self, String> {
        let mut args = Vec::new();
        let mut has_placeholder = false;

        for arg in input {
            // psh: templates are text; an argument that is not UTF-8 is taken lossily.
            let tmpl = FormatTemplate::parse(&String::from_utf8_lossy(&arg));
            has_placeholder |= tmpl.has_tokens();
            args.push(tmpl);
        }

        // We need to check that we have at least one argument, because if not
        // it will try to execute each file and directory it finds.
        if args.is_empty() {
            return Err("No executable provided for --exec or --exec-batch".to_string());
        }

        // A placeholder as the executable is meaningful for `--exec` but never for `--exec-batch`.
        if mode == ExecutionMode::Batch && args[0].has_tokens() {
            return Err(
                "First argument of --exec-batch must be a fixed executable, not a placeholder"
                    .to_string(),
            );
        }

        // If a placeholder token was not supplied, append one at the end of the command.
        if !has_placeholder {
            args.push(FormatTemplate::Tokens(vec![Token::Placeholder]));
        }

        Ok(Self { args })
    }

    fn number_of_tokens(&self) -> usize {
        self.args.iter().filter(|arg| arg.has_tokens()).count()
    }

    /// The command line for one result.
    fn generate(&self, input: &[u8], path_separator: Option<&str>) -> Vec<Vec<u8>> {
        self.args
            .iter()
            .map(|a| a.generate(input, path_separator))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<Vec<u8>> {
        args.iter().map(|a| a.as_bytes().to_vec()).collect()
    }

    fn generate_str(template: &CommandTemplate, input: &str) -> Vec<String> {
        template
            .generate(input.as_bytes(), None)
            .into_iter()
            .map(|a| String::from_utf8(a).unwrap())
            .collect()
    }

    #[test]
    fn tokens_with_placeholder() {
        assert_eq!(
            CommandSet::new(vec![v(&["echo", "${SHELL}:"])]).unwrap(),
            CommandSet {
                commands: vec![CommandTemplate {
                    args: vec![
                        FormatTemplate::Text("echo".into()),
                        FormatTemplate::Text("${SHELL}:".into()),
                        FormatTemplate::Tokens(vec![Token::Placeholder]),
                    ]
                }],
                mode: ExecutionMode::OneByOne,
            }
        );
    }

    #[test]
    fn tokens_multiple() {
        assert_eq!(
            CommandSet::new(vec![v(&["cp", "{}", "{/.}.ext"])]).unwrap(),
            CommandSet {
                commands: vec![CommandTemplate {
                    args: vec![
                        FormatTemplate::Text("cp".into()),
                        FormatTemplate::Tokens(vec![Token::Placeholder]),
                        FormatTemplate::Tokens(vec![
                            Token::BasenameNoExt,
                            Token::Text(".ext".into())
                        ]),
                    ],
                }],
                mode: ExecutionMode::OneByOne,
            }
        );
    }

    #[test]
    fn tokens_with_literal_braces() {
        let template =
            CommandTemplate::new(v(&["{{}}", "{{", "{.}}"]), ExecutionMode::OneByOne).unwrap();
        assert_eq!(
            generate_str(&template, "foo"),
            vec!["{}", "{", "{.}", "foo"]
        );
    }

    #[test]
    fn tokens_with_literal_braces_and_placeholder() {
        let template =
            CommandTemplate::new(v(&["echo", "{{{},end}"]), ExecutionMode::OneByOne).unwrap();
        assert_eq!(generate_str(&template, "foo"), vec!["echo", "{foo,end}"]);
    }

    #[test]
    fn tokens_multiple_batch() {
        assert!(CommandSet::new_batch(vec![v(&["echo", "{.}", "{}"])]).is_err());
    }

    #[test]
    fn command_set_no_args() {
        assert!(CommandSet::new(vec![v(&["echo"]), vec![]]).is_err());
    }

    #[test]
    fn placeholder_as_executable_rejected() {
        assert!(CommandSet::new(vec![v(&["{}"])]).is_ok());
        assert!(CommandSet::new(vec![v(&["{/}", "arg"])]).is_ok());
        assert!(CommandSet::new_batch(vec![v(&["{}"])]).is_err());
    }
}

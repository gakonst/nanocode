use std::future::Future;
use std::io::{BufWriter, Stdout, Write};
use std::pin::Pin;

use nanocode_agent::events::AgentEvent;
use nanocode_agent::{Agent, AgentEvents, TurnControl, TurnResult};
use nanocode_tui::input::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nanocode_tui::terminal::{self, ansi};
use nanocode_tui::unicode;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::markdown::{BOLD, DIM, Markdown, RED, RESET};
use crate::{Config, Lite, Result, Shown, shown, workspace_sessions};

const PROMPT_MARK: &str = "›";
const PROMPT_GAP: &str = " ";
const NEWLINE: &str = "\r\n";
const CLEAR_BELOW: &str = "\x1b[J";
const NEWLINE_GLYPH: char = '↵';
const CONTROL_GLYPH: char = ' ';
const TAB_SPACES: &str = "    ";
const HELP: &str = "/help       show commands and keys
/new        start a fresh session
/sessions   list sessions for this workspace
/exit       quit
Enter submit · Ctrl-C cancel the turn, clear the line or quit · Ctrl-D quit
Up/Down history · Ctrl-A/E start/end · Ctrl-U/K delete to start/end · Ctrl-W delete word";

type TurnFuture = Pin<Box<dyn Future<Output = nanocode_agent::Result<TurnResult>> + Send>>;
type Keys = mpsc::UnboundedReceiver<std::io::Result<Event>>;

pub fn run(config: Config) -> Result<()> {
    let raw = Raw::enter()?;
    let mut out = BufWriter::new(std::io::stdout());
    write!(
        out,
        "{DIM}nc-lite {} · {} · /help for commands{RESET}{NEWLINE}",
        env!("CARGO_PKG_VERSION"),
        config.model
    )?;
    prompt(&mut out)?;
    out.flush()?;
    let (cols, rows) = terminal::size()?;
    crate::tokio_runtime()?.block_on(async move {
        let start = Start::Pending(nanocode_platform::spawn_task(Lite::start(config.clone())));
        let mut tui = Tui {
            out,
            keys: keys(),
            cols: cols.into(),
            rows: rows.into(),
            continue_last: config.continue_last,
            start,
            editor: Editor::default(),
            reply: Reply::default(),
            session: None,
            turn: None,
        };
        let outcome = tui.run().await;
        write!(tui.out, "{NEWLINE}")?;
        tui.out.flush()?;
        drop(raw);
        let closed = tui.close().await;
        outcome.and(closed)
    })
}

struct Raw;

impl Raw {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        write_sequence(ansi::ENABLE_BRACKETED_PASTE)?;
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        Ok(Self)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let _ = write_sequence(ansi::DISABLE_BRACKETED_PASTE);
    let _ = terminal::disable_raw_mode();
}

fn write_sequence(sequence: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    out.write_all(sequence.as_bytes())?;
    out.flush()
}

fn keys() -> Keys {
    let (sender, receiver) = mpsc::unbounded_channel();
    nanocode_platform::spawn_task(async move {
        let mut events = match EventStream::new() {
            Ok(events) => events,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        };
        while let Some(event) = events.next().await {
            let failed = event.is_err();
            if sender.send(event).is_err() || failed {
                return;
            }
        }
    });
    receiver
}

fn prompt(out: &mut impl Write) -> std::io::Result<()> {
    write!(out, "\r{CLEAR_BELOW}{BOLD}{PROMPT_MARK}{RESET}{PROMPT_GAP}")
}

fn prompt_width() -> usize {
    PROMPT_MARK
        .chars()
        .chain(PROMPT_GAP.chars())
        .map(width)
        .sum()
}

const fn glyph(ch: char) -> char {
    match ch {
        '\n' => NEWLINE_GLYPH,
        ch if ch.is_control() => CONTROL_GLYPH,
        ch => ch,
    }
}

fn width(ch: char) -> usize {
    unicode::char_width(ch).unwrap_or_default()
}

fn layout(text: impl Iterator<Item = char>, start: usize, cols: usize) -> (usize, usize) {
    text.fold((0, start), |(row, col), ch| {
        let w = width(ch);
        if col + w > cols {
            (row + 1, w)
        } else {
            (row, col + w)
        }
    })
}

const fn settled((row, col): (usize, usize), cols: usize) -> (usize, usize) {
    if col >= cols {
        (row + 1, 0)
    } else {
        (row, col)
    }
}

fn write_lines(out: &mut impl Write, style: &str, text: &str) -> std::io::Result<()> {
    for line in text.lines() {
        let line: String = line.chars().map(glyph).collect();
        write!(out, "{style}{line}{RESET}{NEWLINE}")?;
    }
    Ok(())
}

enum Start {
    Pending(JoinHandle<Result<Lite>>),
    Ready(Lite),
    Failed(String),
}

struct Session {
    agent: Agent,
    events: AgentEvents,
}

struct Running {
    control: TurnControl,
    result: TurnFuture,
    streamed: bool,
    cancelled: bool,
}

enum Step {
    Idle,
    Edited,
    Exit,
}

struct Tui {
    out: BufWriter<Stdout>,
    keys: Keys,
    cols: usize,
    rows: usize,
    continue_last: bool,
    start: Start,
    editor: Editor,
    reply: Reply,
    session: Option<Session>,
    turn: Option<Running>,
}

async fn next_event(session: &mut Option<Session>) -> Option<AgentEvent> {
    match session {
        Some(session) => session.events.recv().await,
        None => None,
    }
}

async fn turn_result(turn: &mut Option<Running>) -> nanocode_agent::Result<TurnResult> {
    match turn {
        Some(turn) => turn.result.as_mut().await,
        None => std::future::pending().await,
    }
}

impl Tui {
    async fn run(&mut self) -> Result<()> {
        loop {
            let step = nanocode_sync::select! {
                Some(event) = self.keys.recv() => self.keys_batch(event?).await?,
                Some(event) = next_event(&mut self.session) => {
                    self.show(&event)?;
                    self.drain_events()?;
                    Step::Idle
                }
                result = turn_result(&mut self.turn), if self.turn.is_some() => {
                    self.complete(result).await?;
                    Step::Idle
                }
                else => return Err("terminal input closed".into()),
            };
            if let Step::Exit = step {
                return Ok(());
            }
            self.out.flush()?;
        }
    }

    async fn keys_batch(&mut self, first: Event) -> Result<Step> {
        let mut edited = false;
        let mut next = Some(first);
        while let Some(event) = next {
            match self.event(event).await? {
                Step::Exit => return Ok(Step::Exit),
                Step::Edited => edited = true,
                Step::Idle => {}
            }
            next = self.keys.try_recv().ok().transpose()?;
        }
        if edited && self.turn.is_none() {
            self.editor.draw(&mut self.out, self.cols)?;
        }
        Ok(Step::Idle)
    }

    async fn event(&mut self, event: Event) -> Result<Step> {
        match event {
            Event::Paste(text) => {
                self.editor
                    .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                Ok(Step::Edited)
            }
            Event::Resize(cols, rows) => {
                self.resize(cols.into(), rows.into())?;
                Ok(Step::Idle)
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key).await,
            _ => Ok(Step::Idle),
        }
    }

    async fn key(&mut self, key: KeyEvent) -> Result<Step> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let editor = &mut self.editor;
        match key.code {
            KeyCode::Char('c') if control => return self.interrupt().await,
            KeyCode::Char('d') if control && editor.text.is_empty() => return Ok(Step::Exit),
            KeyCode::Char('d') if control => editor.delete(),
            KeyCode::Char('a') if control => editor.home(),
            KeyCode::Char('e') if control => editor.end(),
            KeyCode::Char('b') if control => editor.left(),
            KeyCode::Char('f') if control => editor.right(),
            KeyCode::Char('h') if control => editor.backspace(),
            KeyCode::Char('u') if control => editor.kill_start(),
            KeyCode::Char('k') if control => editor.kill_end(),
            KeyCode::Char('w') if control => editor.kill_word(),
            KeyCode::Char('p') if control => editor.up(),
            KeyCode::Char('n') if control => editor.down(),
            KeyCode::Char(_) if control || alt => return Ok(Step::Idle),
            KeyCode::Char(ch) => editor.insert(ch.encode_utf8(&mut [0; 4])),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Left => editor.left(),
            KeyCode::Right => editor.right(),
            KeyCode::Home => editor.home(),
            KeyCode::End => editor.end(),
            KeyCode::Up => editor.up(),
            KeyCode::Down => editor.down(),
            KeyCode::Enter => return self.enter().await,
            _ => return Ok(Step::Idle),
        }
        Ok(Step::Edited)
    }

    async fn interrupt(&mut self) -> Result<Step> {
        if let Some(turn) = &mut self.turn {
            if !turn.cancelled {
                turn.cancelled = true;
                let control = turn.control.clone();
                nanocode_platform::spawn_task(async move { control.cancel().await });
            }
            return Ok(Step::Idle);
        }
        if self.editor.text.is_empty() {
            return Ok(Step::Exit);
        }
        self.editor.clear();
        Ok(Step::Edited)
    }

    fn resize(&mut self, cols: usize, rows: usize) -> Result<()> {
        let before = self.cols;
        (self.cols, self.rows) = (cols, rows);
        if self.turn.is_some() {
            self.reply.resized = true;
            return Ok(());
        }
        let span = prompt_width() + self.editor.text.chars().map(width).sum::<usize>();
        if span >= before.min(cols) {
            self.editor.leave(&mut self.out, before)?;
        }
        self.editor.draw(&mut self.out, cols)?;
        Ok(())
    }

    async fn enter(&mut self) -> Result<Step> {
        if self.turn.is_some() {
            return Ok(Step::Idle);
        }
        self.editor.leave(&mut self.out, self.cols)?;
        let line = self.editor.take();
        let outcome = match line.trim() {
            "" => Ok(()),
            "/help" => write_lines(&mut self.out, DIM, HELP).map_err(Into::into),
            "/exit" | "/quit" => return Ok(Step::Exit),
            "/new" => self.new_session().await,
            "/sessions" => self.list_sessions().await,
            command if command.starts_with('/') && !command.contains(char::is_whitespace) => {
                Err(format!("unknown command {command}; /help lists them").into())
            }
            _ => match self.start_turn(&line).await {
                Ok(()) => return Ok(Step::Idle),
                Err(error) => Err(error),
            },
        };
        if let Err(error) = outcome {
            write_lines(&mut self.out, RED, &format!("error: {error}"))?;
        }
        Ok(Step::Edited)
    }

    async fn lite(&mut self) -> Result<&Lite> {
        if let Start::Pending(handle) = &mut self.start {
            self.start = match handle.await {
                Ok(Ok(lite)) => Start::Ready(lite),
                Ok(Err(error)) => Start::Failed(error.to_string()),
                Err(error) => Start::Failed(error.to_string()),
            };
        }
        match &self.start {
            Start::Ready(lite) => Ok(lite),
            Start::Failed(error) => Err(error.clone().into()),
            Start::Pending(_) => Err("the runtime is still starting".into()),
        }
    }

    async fn start_turn(&mut self, text: &str) -> Result<()> {
        if self.session.is_none() {
            let continue_last = self.continue_last;
            let (agent, events) = self.lite().await?.session(continue_last).await?;
            self.continue_last = false;
            self.session = Some(Session { agent, events });
        }
        let Some(session) = &self.session else {
            return Ok(());
        };
        let turn = session.agent.prompt(text).await?;
        self.turn = Some(Running {
            control: turn.control(),
            result: Box::pin(turn.result()),
            streamed: false,
            cancelled: false,
        });
        Ok(())
    }

    async fn new_session(&mut self) -> Result<()> {
        self.continue_last = false;
        if let Some(session) = self.session.take() {
            let id = session.agent.session_id().to_owned();
            session.agent.flush().await?;
            write_lines(&mut self.out, DIM, &format!("closed session {id}"))?;
        }
        write_lines(&mut self.out, DIM, "the next prompt starts a fresh session")?;
        Ok(())
    }

    async fn list_sessions(&mut self) -> Result<()> {
        let current = self
            .session
            .as_ref()
            .map(|session| session.agent.session_id().to_owned());
        let lines: Vec<String> = match &self.lite().await?.store {
            Some(store) => workspace_sessions(store)
                .await?
                .into_iter()
                .map(|session| {
                    let id = session.record.session_id;
                    let mark = if current.as_deref() == Some(id.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    format!("{mark} {id}  {}", session.preview.unwrap_or_default())
                })
                .collect(),
            None => vec!["sessions are not saved with --ephemeral".into()],
        };
        if lines.is_empty() {
            write_lines(&mut self.out, DIM, "no sessions for this workspace")?;
        }
        for line in lines {
            write_lines(&mut self.out, DIM, &line.replace('\n', " "))?;
        }
        Ok(())
    }

    fn show(&mut self, event: &AgentEvent) -> std::io::Result<()> {
        let Some(turn) = &mut self.turn else {
            return Ok(());
        };
        match shown(event) {
            Some(Shown::Text(text)) => {
                turn.streamed = true;
                self.reply.push(&mut self.out, &text, self.cols, self.rows)
            }
            Some(Shown::Tool(tool)) => {
                self.reply.end(&mut self.out, self.cols, self.rows)?;
                write_lines(&mut self.out, DIM, &format!("· {tool}"))
            }
            None => Ok(()),
        }
    }

    fn drain_events(&mut self) -> std::io::Result<()> {
        while let Some(event) = self.session.as_mut().and_then(|s| s.events.try_recv()) {
            self.show(&event)?;
        }
        Ok(())
    }

    async fn complete(&mut self, result: nanocode_agent::Result<TurnResult>) -> Result<()> {
        self.drain_events()?;
        let Some(turn) = self.turn.take() else {
            return Ok(());
        };
        let failure = match result {
            Ok(result) if !turn.streamed => {
                self.reply
                    .push(&mut self.out, result.final_message(), self.cols, self.rows)?;
                None
            }
            Ok(_) => None,
            Err(_) if turn.cancelled => None,
            Err(error) => Some(error),
        };
        self.reply.end(&mut self.out, self.cols, self.rows)?;
        self.reply.markdown.reset();
        if turn.cancelled {
            write_lines(&mut self.out, DIM, "· cancelled")?;
        }
        if let Some(error) = failure {
            write_lines(&mut self.out, RED, &format!("error: {error}"))?;
        }
        write!(self.out, "{NEWLINE}")?;
        self.editor.draw(&mut self.out, self.cols)?;
        Ok(())
    }

    async fn close(self) -> Result<()> {
        if let Some(turn) = &self.turn {
            turn.control.cancel().await?;
        }
        let flushed = match &self.session {
            Some(session) => {
                eprintln!("session {}", session.agent.session_id());
                session.agent.flush().await.map_err(Into::into)
            }
            None => Ok(()),
        };
        let lite = match self.start {
            Start::Pending(handle) => handle.await?,
            Start::Ready(lite) => Ok(lite),
            Start::Failed(_) => return flushed,
        };
        let shutdown = match lite {
            Ok(lite) => lite.shutdown().await,
            Err(_) => Ok(()),
        };
        flushed.and(shutdown)
    }
}

#[derive(Default)]
struct Editor {
    text: String,
    cursor: usize,
    row: usize,
    history: Vec<String>,
    recalled: Option<usize>,
    draft: String,
}

impl Editor {
    fn insert(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, |ch| self.cursor - ch.len_utf8())
    }

    fn following(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |ch| self.cursor + ch.len_utf8())
    }

    fn backspace(&mut self) {
        let start = self.previous();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn delete(&mut self) {
        let end = self.following();
        self.text.replace_range(self.cursor..end, "");
    }

    fn left(&mut self) {
        self.cursor = self.previous();
    }

    fn right(&mut self) {
        self.cursor = self.following();
    }

    const fn home(&mut self) {
        self.cursor = 0;
    }

    const fn end(&mut self) {
        self.cursor = self.text.len();
    }

    fn kill_start(&mut self) {
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
    }

    fn kill_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    fn kill_word(&mut self) {
        let before = &self.text[..self.cursor];
        let word = before.trim_end_matches(char::is_whitespace);
        let start = word
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
            .map_or(0, |(at, ch)| at + ch.len_utf8());
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.recalled = None;
    }

    fn recall(&mut self, index: Option<usize>) {
        self.text = match index {
            Some(index) => self.history[index].clone(),
            None => std::mem::take(&mut self.draft),
        };
        self.recalled = index;
        self.cursor = self.text.len();
    }

    fn up(&mut self) {
        let index = match self.recalled {
            Some(index) => index.saturating_sub(1),
            None if self.history.is_empty() => return,
            None => {
                self.draft = self.text.clone();
                self.history.len() - 1
            }
        };
        self.recall(Some(index));
    }

    fn down(&mut self) {
        match self.recalled {
            Some(index) if index + 1 < self.history.len() => self.recall(Some(index + 1)),
            Some(_) => self.recall(None),
            None => {}
        }
    }

    fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.recalled = None;
        self.draft.clear();
        if !text.trim().is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        text
    }

    fn end_position(&self, cols: usize) -> (usize, usize) {
        settled(
            layout(self.text.chars().map(glyph), prompt_width(), cols),
            cols,
        )
    }

    fn draw(&mut self, out: &mut impl Write, cols: usize) -> std::io::Result<()> {
        if self.row > 0 {
            write!(out, "\x1b[{}A", self.row)?;
        }
        prompt(out)?;
        let shown: String = self.text.chars().map(glyph).collect();
        out.write_all(shown.as_bytes())?;
        let (end_row, end_col) = layout(shown.chars(), prompt_width(), cols);
        let mut row = end_row;
        if end_col >= cols {
            write!(out, "{NEWLINE}")?;
            row += 1;
        }
        let before = self.text[..self.cursor].chars().map(glyph);
        let (target_row, target_col) = settled(layout(before, prompt_width(), cols), cols);
        if row > target_row {
            write!(out, "\x1b[{}A", row - target_row)?;
        }
        out.write_all(b"\r")?;
        if target_col > 0 {
            write!(out, "\x1b[{target_col}C")?;
        }
        self.row = target_row;
        Ok(())
    }

    fn leave(&mut self, out: &mut impl Write, cols: usize) -> std::io::Result<()> {
        let (end_row, _) = self.end_position(cols);
        if end_row > self.row {
            write!(out, "\x1b[{}B", end_row - self.row)?;
        }
        write!(out, "{NEWLINE}")?;
        self.row = 0;
        Ok(())
    }
}

#[derive(Default)]
struct Reply {
    line: String,
    resized: bool,
    markdown: Markdown,
}

impl Reply {
    fn push(
        &mut self,
        out: &mut impl Write,
        text: &str,
        cols: usize,
        rows: usize,
    ) -> std::io::Result<()> {
        for (index, piece) in text.split('\n').enumerate() {
            if index > 0 {
                self.newline(out, cols, rows)?;
            }
            if piece.contains(char::is_control) {
                let clean: String = piece
                    .chars()
                    .filter(|ch| *ch == '\t' || !ch.is_control())
                    .collect::<String>()
                    .replace('\t', TAB_SPACES);
                self.write(out, &clean)?;
            } else {
                self.write(out, piece)?;
            }
        }
        Ok(())
    }

    fn write(&mut self, out: &mut impl Write, piece: &str) -> std::io::Result<()> {
        out.write_all(piece.as_bytes())?;
        self.line.push_str(piece);
        Ok(())
    }

    fn end(&mut self, out: &mut impl Write, cols: usize, rows: usize) -> std::io::Result<()> {
        if self.line.is_empty() {
            return Ok(());
        }
        self.newline(out, cols, rows)
    }

    fn newline(&mut self, out: &mut impl Write, cols: usize, rows: usize) -> std::io::Result<()> {
        let styled = self.markdown.line(&self.line, cols);
        let plain = styled.strip_suffix(RESET) == Some(self.line.as_str());
        let (up, _) = layout(self.line.chars(), 0, cols);
        if !plain && !self.resized && up < rows {
            if up > 0 {
                write!(out, "\x1b[{up}A")?;
            }
            write!(out, "\r{CLEAR_BELOW}{styled}")?;
        }
        write!(out, "{NEWLINE}")?;
        self.line.clear();
        self.resized = false;
        Ok(())
    }
}

use crate::postgres::{
    DatabaseError, PgError, PgOptions,
    scram::{self, Scram},
    stream::Stream,
    wire::{self, Body, Inbox, tag},
};
use nanocode_durability_core::{Settle, SqlRow, SqlStatement};
use nanocode_sync::SwapOption;
use rustls::ClientConfig;
use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
};

const STATEMENTS_CACHED_MAX: usize = 100;
const STATEMENT_PREFIX: &str = "nanocode_";
const AUTHENTICATION_OK: i32 = 0;
const AUTHENTICATION_CLEARTEXT: i32 = 3;
const AUTHENTICATION_MD5: i32 = 5;
const AUTHENTICATION_SASL: i32 = 10;
const AUTHENTICATION_SASL_CONTINUE: i32 = 11;
const AUTHENTICATION_SASL_FINAL: i32 = 12;
const IDLE: u8 = b'I';
const BEGIN: &str = "BEGIN";
const UNSYNCED: &str = "SET LOCAL synchronous_commit = off";
const COMMIT: &str = "COMMIT";
const ROLLBACK: &str = "ROLLBACK";
const SESSION: [(&str, &str); 4] = [
    ("DateStyle", "ISO, MDY"),
    ("client_encoding", "UTF8"),
    ("TimeZone", "UTC"),
    ("extra_float_digits", "2"),
];

#[derive(Default)]
pub(super) struct Reuse {
    pub(super) tls: SwapOption<ClientConfig>,
    salted: SwapOption<scram::Salted>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Ready,
    Busy,
    Broken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub process_id: u32,
    pub channel: String,
    pub payload: String,
}

type StatementKey = (&'static str, usize);

pub struct PgConnection {
    stream: Stream,
    inbox: Inbox,
    out: Vec<u8>,
    statements: HashMap<StatementKey, usize>,
    name: String,
    notifications: VecDeque<Notification>,
    process_id: u32,
    phase: Phase,
}

struct Answer {
    results: Vec<Vec<SqlRow>>,
    error: Option<DatabaseError>,
    status: u8,
}

impl PgConnection {
    pub async fn connect(options: &PgOptions) -> Result<Self, PgError> {
        Self::connect_reusing(options, &Reuse::default()).await
    }

    pub(super) async fn connect_reusing(
        options: &PgOptions,
        reuse: &Reuse,
    ) -> Result<Self, PgError> {
        let stream = Stream::connect(options, &reuse.tls).await?;
        let mut connection = Self {
            stream,
            inbox: Inbox::new(),
            out: Vec::with_capacity(wire::READ_BUFFER_BYTES),
            statements: HashMap::new(),
            name: String::new(),
            notifications: VecDeque::new(),
            process_id: 0,
            phase: Phase::Busy,
        };
        connection.start(options, reuse).await?;
        connection.phase = Phase::Ready;
        Ok(connection)
    }

    #[must_use]
    pub const fn process_id(&self) -> u32 {
        self.process_id
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.phase == Phase::Ready
    }

    async fn start(&mut self, options: &PgOptions, reuse: &Reuse) -> Result<(), PgError> {
        let mut parameters = vec![
            ("user", options.user.as_str()),
            ("database", options.database.as_str()),
        ];
        parameters.extend(SESSION);
        if let Some(name) = &options.application_name {
            parameters.push(("application_name", name));
        }
        self.out.clear();
        wire::startup(&mut self.out, &parameters)?;
        self.flush().await?;
        self.authenticate(options, reuse).await?;
        loop {
            let (code, range) = self.receive().await?;
            let mut body = self.inbox.body(range);
            match code {
                tag::BACKEND_KEY => self.process_id = body.i32()?.cast_unsigned(),
                tag::PARAMETER_STATUS | tag::NOTICE => {}
                tag::ERROR => return Err(PgError::Database(body.error()?)),
                tag::READY => return Ok(()),
                code => return Err(unexpected(code, "starting a session")),
            }
        }
    }

    async fn authenticate(&mut self, options: &PgOptions, reuse: &Reuse) -> Result<(), PgError> {
        let password = || {
            options.password.as_deref().ok_or_else(|| {
                PgError::Authentication(
                    "the server asks for a password and none was given".to_owned(),
                )
            })
        };
        let (kind, mechanisms) = self.challenge().await?;
        match kind {
            AUTHENTICATION_OK => return Ok(()),
            AUTHENTICATION_CLEARTEXT => {
                self.out.clear();
                wire::password(&mut self.out, password()?)?;
                self.flush().await?;
            }
            AUTHENTICATION_SASL => {
                if !mechanisms
                    .iter()
                    .any(|mechanism| mechanism == scram::MECHANISM)
                {
                    return Err(PgError::Authentication(format!(
                        "the server offers {mechanisms:?}, and this driver speaks only {}",
                        scram::MECHANISM
                    )));
                }
                self.scram(password()?, &reuse.salted).await?;
            }
            AUTHENTICATION_MD5 => {
                return Err(PgError::Authentication(
                    "the server asks for an md5 password; this driver speaks scram-sha-256"
                        .to_owned(),
                ));
            }
            kind => {
                return Err(PgError::Authentication(format!(
                    "the server asks for authentication method {kind}, which this driver does not speak"
                )));
            }
        }
        match self.challenge().await? {
            (AUTHENTICATION_OK, _) => Ok(()),
            (kind, _) => Err(unexpected_authentication(kind)),
        }
    }

    async fn scram(
        &mut self,
        password: &str,
        reused: &SwapOption<scram::Salted>,
    ) -> Result<(), PgError> {
        let (scram, first) = Scram::start()?;
        self.out.clear();
        wire::sasl_initial(&mut self.out, scram::MECHANISM, first.as_bytes())?;
        self.flush().await?;
        let server_first = self.sasl(AUTHENTICATION_SASL_CONTINUE).await?;
        let proof = scram.prove(password, &server_first, reused)?;
        self.out.clear();
        wire::sasl_response(&mut self.out, proof.message.as_bytes());
        self.flush().await?;
        let server_final = self.sasl(AUTHENTICATION_SASL_FINAL).await?;
        proof.verify(&server_final)
    }

    async fn sasl(&mut self, expected: i32) -> Result<Vec<u8>, PgError> {
        let (code, range) = self.receive_skipping_notices().await?;
        let mut body = self.inbox.body(range);
        match code {
            tag::AUTHENTICATION => {
                let kind = body.i32()?;
                if kind != expected {
                    return Err(unexpected_authentication(kind));
                }
                Ok(body.rest().to_vec())
            }
            tag::ERROR => Err(PgError::Database(body.error()?)),
            code => Err(unexpected(code, "authenticating")),
        }
    }

    async fn challenge(&mut self) -> Result<(i32, Vec<String>), PgError> {
        let (code, range) = self.receive_skipping_notices().await?;
        let mut body = self.inbox.body(range);
        match code {
            tag::AUTHENTICATION => {
                let kind = body.i32()?;
                let mut mechanisms = Vec::new();
                if kind == AUTHENTICATION_SASL {
                    loop {
                        let mechanism = body.cstr()?;
                        if mechanism.is_empty() {
                            break;
                        }
                        mechanisms.push(mechanism.to_owned());
                    }
                }
                Ok((kind, mechanisms))
            }
            tag::ERROR => Err(PgError::Database(body.error()?)),
            code => Err(unexpected(code, "authenticating")),
        }
    }

    pub async fn transaction(
        &mut self,
        settle: Settle,
        statements: &[SqlStatement<'_>],
    ) -> Result<Vec<Vec<SqlRow>>, PgError> {
        let begin: &[&'static str] = match settle {
            Settle::Durable => &[BEGIN],
            Settle::Ordered => &[BEGIN, UNSYNCED],
        };
        let answer = self.pipeline(begin, statements, &[COMMIT]).await?;
        if let Some(error) = answer.error {
            if answer.status != IDLE {
                self.simple(ROLLBACK).await?;
            }
            return Err(PgError::Database(error));
        }
        if answer.status != IDLE {
            return Err(self.broken(PgError::Protocol(format!(
                "a committed transaction left the session in state {}",
                answer.status as char
            ))));
        }
        Ok(answer.results)
    }

    pub async fn run(
        &mut self,
        statements: &[SqlStatement<'_>],
    ) -> Result<Vec<Vec<SqlRow>>, PgError> {
        let answer = self.pipeline(&[], statements, &[]).await?;
        match answer.error {
            Some(error) => Err(PgError::Database(error)),
            None => Ok(answer.results),
        }
    }

    async fn pipeline(
        &mut self,
        before: &[&'static str],
        statements: &[SqlStatement<'_>],
        after: &[&'static str],
    ) -> Result<Answer, PgError> {
        self.enter()?;
        self.out.clear();
        let mut parsed: Vec<StatementKey> = Vec::new();
        let mut parses: Vec<Option<usize>> = Vec::new();
        let bare: &[&str] = &[];
        let sent = before
            .iter()
            .map(|&sql| (sql, bare))
            .chain(
                statements
                    .iter()
                    .map(|statement| (statement.sql, statement.params.as_slice())),
            )
            .chain(after.iter().map(|&sql| (sql, bare)));
        let mut executes = 0;
        for (sql, params) in sent {
            if let Err(error) = self.prepare(sql, params.len(), &mut parsed, &mut parses) {
                self.phase = Phase::Ready;
                return Err(error);
            }
            if let Err(error) = wire::bind(&mut self.out, &self.name, params) {
                self.phase = Phase::Ready;
                return Err(error);
            }
            wire::execute(&mut self.out);
            executes += 1;
        }
        wire::sync(&mut self.out);
        self.flush().await?;
        let mut answer = self.answer(&parsed, &parses).await?;
        if answer.error.is_none() && answer.results.len() != executes {
            return Err(self.broken(PgError::Protocol(format!(
                "{} results answered {executes} statements",
                answer.results.len()
            ))));
        }
        if answer.error.is_none() {
            answer.results.truncate(executes - after.len());
            answer.results.drain(..before.len());
        }
        Ok(answer)
    }

    fn prepare(
        &mut self,
        sql: &'static str,
        parameters: usize,
        parsed: &mut Vec<StatementKey>,
        parses: &mut Vec<Option<usize>>,
    ) -> Result<(), PgError> {
        let key = (sql, parameters);
        self.name.clear();
        if let Some(&at) = self.statements.get(&key) {
            write!(self.name, "{STATEMENT_PREFIX}{at}").expect("writing to a String succeeds");
            return Ok(());
        }
        if let Some(at) = parsed.iter().position(|parsing| *parsing == key) {
            let at = self.statements.len() + at;
            write!(self.name, "{STATEMENT_PREFIX}{at}").expect("writing to a String succeeds");
            return Ok(());
        }
        if self.statements.len() + parsed.len() < STATEMENTS_CACHED_MAX {
            let at = self.statements.len() + parsed.len();
            write!(self.name, "{STATEMENT_PREFIX}{at}").expect("writing to a String succeeds");
            parses.push(Some(parsed.len()));
            parsed.push(key);
        } else {
            parses.push(None);
        }
        wire::parse(&mut self.out, &self.name, sql, parameters)
    }

    async fn answer(
        &mut self,
        parsed: &[StatementKey],
        parses: &[Option<usize>],
    ) -> Result<Answer, PgError> {
        let mut answer = Answer {
            results: Vec::new(),
            error: None,
            status: IDLE,
        };
        let mut rows = Vec::new();
        let mut parse_completes = parses.iter();
        let first = self.statements.len();
        loop {
            let (code, range) = self.receive().await?;
            let body = self.inbox.body(range);
            match code {
                tag::PARSE_COMPLETE => {
                    let Some(&parse) = parse_completes.next() else {
                        return Err(self.broken(unexpected(code, "after every parse completed")));
                    };
                    if let Some(at) = parse {
                        let previous = self.statements.insert(parsed[at], first + at);
                        assert!(
                            previous.is_none(),
                            "a statement is prepared once per connection"
                        );
                    }
                }
                tag::DATA_ROW => rows.push(body.row().map_err(|error| self.broken(error))?),
                tag::COMMAND_COMPLETE | tag::EMPTY_QUERY => {
                    answer.results.push(std::mem::take(&mut rows));
                }
                tag::ERROR => {
                    let error = body.error().map_err(|error| self.broken(error))?;
                    answer.error.get_or_insert(error);
                }
                tag::READY => {
                    answer.status = body.rest().first().copied().unwrap_or_default();
                    self.phase = Phase::Ready;
                    return Ok(answer);
                }
                tag::NOTIFICATION => {
                    let notification = notification(body).map_err(|error| self.broken(error))?;
                    self.notifications.push_back(notification);
                }
                tag::BIND_COMPLETE
                | tag::CLOSE_COMPLETE
                | tag::NO_DATA
                | tag::NOTICE
                | tag::PARAMETER_STATUS
                | tag::PARAMETER_DESCRIPTION
                | tag::ROW_DESCRIPTION => {}
                code => return Err(self.broken(unexpected(code, "answering a pipeline"))),
            }
        }
    }

    pub async fn simple(&mut self, sql: &str) -> Result<Vec<Vec<SqlRow>>, PgError> {
        self.enter()?;
        self.out.clear();
        if let Err(error) = wire::query(&mut self.out, sql) {
            self.phase = Phase::Ready;
            return Err(error);
        }
        self.flush().await?;
        let answer = self.answer(&[], &[]).await?;
        match answer.error {
            Some(error) => Err(PgError::Database(error)),
            None => Ok(answer.results),
        }
    }

    pub async fn listen(&mut self, channel: &str) -> Result<(), PgError> {
        self.simple(&format!("LISTEN \"{}\"", channel.replace('"', "\"\"")))
            .await?;
        Ok(())
    }

    pub async fn notification(&mut self) -> Result<Notification, PgError> {
        if let Some(notification) = self.notifications.pop_front() {
            return Ok(notification);
        }
        if self.phase != Phase::Ready {
            return Err(self.broken(PgError::Broken));
        }
        loop {
            let (code, range) = self.receive().await?;
            let body = self.inbox.body(range);
            match code {
                tag::NOTIFICATION => return notification(body).map_err(|error| self.broken(error)),
                tag::NOTICE | tag::PARAMETER_STATUS => {}
                tag::ERROR => {
                    let error = body.error().map_err(|error| self.broken(error))?;
                    return Err(self.broken(PgError::Database(error)));
                }
                code => return Err(self.broken(unexpected(code, "waiting for a notification"))),
            }
        }
    }

    pub async fn close(mut self) -> Result<(), PgError> {
        self.out.clear();
        wire::terminate(&mut self.out);
        self.flush().await
    }

    const fn enter(&mut self) -> Result<(), PgError> {
        match self.phase {
            Phase::Ready => {
                self.phase = Phase::Busy;
                Ok(())
            }
            Phase::Busy | Phase::Broken => Err(self.broken(PgError::Broken)),
        }
    }

    const fn broken(&mut self, error: PgError) -> PgError {
        self.phase = Phase::Broken;
        error
    }

    async fn flush(&mut self) -> Result<(), PgError> {
        if let Err(error) = self.stream.write_all(&self.out).await {
            return Err(self.broken(error.into()));
        }
        Ok(())
    }

    async fn receive(&mut self) -> Result<(u8, std::ops::Range<usize>), PgError> {
        loop {
            match self.inbox.next() {
                Ok(Some(message)) => return Ok(message),
                Ok(None) => {}
                Err(error) => return Err(self.broken(error)),
            }
            match self.stream.read_buf(self.inbox.room()).await {
                Ok(0) => {
                    return Err(self.broken(PgError::Io(std::io::ErrorKind::UnexpectedEof.into())));
                }
                Ok(_) => {}
                Err(error) => return Err(self.broken(error.into())),
            }
        }
    }

    async fn receive_skipping_notices(&mut self) -> Result<(u8, std::ops::Range<usize>), PgError> {
        loop {
            let (code, range) = self.receive().await?;
            if code != tag::NOTICE {
                return Ok((code, range));
            }
        }
    }
}

fn notification(mut body: Body<'_>) -> Result<Notification, PgError> {
    Ok(Notification {
        process_id: body.i32()?.cast_unsigned(),
        channel: body.cstr()?.to_owned(),
        payload: body.cstr()?.to_owned(),
    })
}

fn unexpected(code: u8, during: &str) -> PgError {
    PgError::Protocol(format!(
        "unexpected message {:?} while {during}",
        code as char
    ))
}

fn unexpected_authentication(kind: i32) -> PgError {
    PgError::Authentication(format!("unexpected authentication message {kind}"))
}

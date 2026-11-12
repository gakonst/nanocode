use std::{collections::HashMap, time::Instant};

use nanocode::agent::events::{AgentEvent, AgentEventKind};
use nanocode_observability::{METRICS_TARGET, TUI_TARGET};
use nanocode_tui::input::{Event, KeyCode, KeyModifiers, MouseEventKind};
use opentelemetry::{Context, trace::TraceContextExt};
use serde_json::Value;
use tracing::{Level, Span, field::Empty};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub fn input_span(event: &Event) -> Span {
    let action = match event {
        Event::Key(key) => match key.code {
            KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                match character {
                    'c' => "clear_or_quit",
                    'z' => "restore_draft",
                    'd' => "model_picker",
                    's' => "thinking_dial",
                    'o' => "display_mode",
                    't' => "agent_tree",
                    'r' => "recent_prompts",
                    'g' => "external_editor",
                    'l' => "redraw",
                    'v' => "paste_image",
                    _ => "control_key",
                }
            }
            KeyCode::Char(_) => "text",
            KeyCode::Enter => "submit",
            KeyCode::Esc => "escape",
            KeyCode::Tab | KeyCode::BackTab => "tab",
            KeyCode::PageUp => "page_up",
            KeyCode::PageDown => "page_down",
            KeyCode::Home => "home",
            KeyCode::End => "end",
            KeyCode::F(1) => "help",
            KeyCode::F(2) => "agent_focus",
            KeyCode::Up => "up",
            KeyCode::Down => "down",
            KeyCode::Left => "left",
            KeyCode::Right => "right",
            KeyCode::Backspace | KeyCode::Delete => "delete",
            _ => "key",
        },
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollUp => "scroll_up",
            MouseEventKind::ScrollDown => "scroll_down",
            MouseEventKind::Down(_) => "pane_click",
            MouseEventKind::Drag(_) => "select",
            MouseEventKind::Up(_) => "release",
            _ => "mouse",
        },
        Event::Resize(..) => "resize",
        Event::Paste(_) => "paste",
        Event::FocusGained => "focus_gained",
        Event::FocusLost => "focus_lost",
    };
    tracing::info_span!(target: TUI_TARGET, "nc.input", input.action = action, input.redraw = Empty, session.id = Empty)
}

#[derive(Default)]
pub struct TurnSpans {
    turn: Option<Span>,
    model: Option<(Span, Instant, bool)>,
    tools: HashMap<String, Span>,
    model_calls: u64,
    tool_calls: u64,
    steers: u64,
}

impl TurnSpans {
    pub fn begin(&mut self, request_id: &str, session_id: Option<&str>) -> Span {
        self.finish(None);
        let span = tracing::info_span!(
            target: "nanocode",
            "nc.turn",
            turn.request_id = request_id,
            session.id = session_id.unwrap_or_default(),
            turn.outcome = Empty,
            turn.model_calls = Empty,
            turn.tool_calls = Empty,
            turn.steers = Empty,
            otel.status_code = Empty,
        );
        if !span.is_disabled() {
            self.turn = Some(span.clone());
        }
        span
    }

    pub fn event(&mut self, event: &AgentEvent, session_id: Option<&str>) {
        if self.turn.is_none() {
            if event.kind != AgentEventKind::RunStarted
                || !tracing::enabled!(target: "nanocode", tracing::Level::INFO)
            {
                return;
            }
            let span = self.begin(&event.request_id, session_id);
            if span.is_disabled() {
                return;
            }
        }
        let Some(turn) = self.turn.clone() else {
            return;
        };
        match event.kind {
            AgentEventKind::ModelCallStarted => {
                let payload = payload(event);
                let span = tracing::info_span!(
                    target: "nanocode",
                    parent: &turn,
                    "nc.model_call",
                    model = payload["model"].as_str().unwrap_or_default(),
                    model_call.index = payload["call_index"].as_u64(),
                    model_call.first_token_ms = Empty,
                    usage.input_tokens = Empty,
                    usage.output_tokens = Empty,
                    usage.cached_tokens = Empty,
                    otel.status_code = Empty,
                );
                self.model = Some((span, Instant::now(), false));
            }
            AgentEventKind::AssistantDelta | AgentEventKind::ReasoningSummaryDelta => {
                if let Some((span, started, seen)) = &mut self.model
                    && !*seen
                {
                    *seen = true;
                    span.record(
                        "model_call.first_token_ms",
                        started.elapsed().as_secs_f64() * 1_000.0,
                    );
                }
            }
            AgentEventKind::ModelCallCompleted | AgentEventKind::ModelCallFailed => {
                self.model_calls += 1;
                if let Some((span, _, _)) = self.model.take() {
                    let payload = payload(event);
                    let usage = &payload["usage"];
                    span.record("usage.input_tokens", usage["input_tokens"].as_u64());
                    span.record("usage.output_tokens", usage["output_tokens"].as_u64());
                    span.record(
                        "usage.cached_tokens",
                        usage["input_tokens_details"]["cached_tokens"].as_u64(),
                    );
                    if event.kind == AgentEventKind::ModelCallFailed {
                        span.record("otel.status_code", "ERROR");
                    }
                }
            }
            AgentEventKind::ToolCall => {
                let payload = payload(event);
                self.tool_calls += 1;
                let call_id = payload["call_id"].as_str().unwrap_or_default().to_owned();
                let parent = payload["parent_call_id"]
                    .as_str()
                    .or_else(|| call_id.rsplit_once('/').map(|(parent, _)| parent))
                    .and_then(|parent| self.tools.get(parent))
                    .unwrap_or(&turn);
                let span = tracing::info_span!(
                    target: "nanocode",
                    parent: parent,
                    "nc.tool_call",
                    tool.name = payload["tool"].as_str().unwrap_or_default(),
                    tool.call_id = call_id.as_str(),
                    tool.status = Empty,
                    tool.duration_ns = Empty,
                    otel.status_code = Empty,
                );
                self.tools.insert(call_id, span);
            }
            AgentEventKind::ToolResult => {
                let payload = payload(event);
                if let Some(span) = payload["call_id"]
                    .as_str()
                    .and_then(|call_id| self.tools.remove(call_id))
                {
                    let status = payload["status"].as_str().unwrap_or_default();
                    span.record("tool.status", status);
                    span.record("tool.duration_ns", payload["duration_ns"].as_u64());
                    if status != "completed" {
                        span.record("otel.status_code", "ERROR");
                    }
                }
            }
            AgentEventKind::RunSteered => {
                self.steers += 1;
                tracing::info!(target: "nanocode", parent: &turn, "steer delivered");
            }
            AgentEventKind::RunCompleted | AgentEventKind::RunFailed => {
                let payload = payload(event);
                let status = payload["status"].as_str().unwrap_or("failed").to_owned();
                self.finish(Some(&status));
            }
            _ => {}
        }
    }

    fn finish(&mut self, outcome: Option<&str>) {
        self.model = None;
        self.tools.clear();
        let counts = (
            std::mem::take(&mut self.model_calls),
            std::mem::take(&mut self.tool_calls),
            std::mem::take(&mut self.steers),
        );
        let Some(turn) = self.turn.take() else {
            return;
        };
        let Some(outcome) = outcome else {
            turn.record("turn.outcome", "superseded");
            return;
        };
        turn.record("turn.outcome", outcome);
        turn.record("turn.model_calls", counts.0);
        turn.record("turn.tool_calls", counts.1);
        turn.record("turn.steers", counts.2);
        if outcome != "completed" {
            turn.record("otel.status_code", "ERROR");
        }
    }
}

fn payload(event: &AgentEvent) -> Value {
    serde_json::from_str(event.payload.get()).unwrap_or(Value::Null)
}

pub struct FrameMetrics {
    enabled: bool,
    input: Option<Instant>,
    stream: Option<Instant>,
    resize: Option<Instant>,
    input_context: Option<Context>,
}

impl FrameMetrics {
    pub fn new() -> Self {
        Self {
            enabled: tracing::enabled!(target: METRICS_TARGET, Level::INFO),
            input: None,
            stream: None,
            resize: None,
            input_context: None,
        }
    }

    pub fn wakeup(&self) {
        if self.enabled {
            tracing::event!(target: METRICS_TARGET, Level::INFO, { monotonic_counter.nc.loop.wakeups = 1_u64 });
        }
    }

    pub fn input(&mut self, resize: bool, span: &Span) {
        if self.enabled {
            let now = Instant::now();
            self.input.get_or_insert(now);
            self.input_context.get_or_insert_with(|| span.context());
            if resize {
                self.resize.get_or_insert(now);
                tracing::event!(target: METRICS_TARGET, Level::INFO, { monotonic_counter.nc.resize.events = 1_u64 });
            }
        }
    }

    pub fn stream(&mut self) {
        if self.enabled {
            self.stream.get_or_insert_with(Instant::now);
        }
    }

    pub fn frame(&mut self, session: Option<&str>) -> Span {
        let span = tracing::info_span!(target: TUI_TARGET, "nc.frame", session.id = session, frame.input = self.input.is_some(), frame.stream = self.stream.is_some(), frame.resize = self.resize.is_some(), frame.input_ms = Empty, frame.stream_ms = Empty, frame.resize_ms = Empty, thread.cpu_ns = Empty, otel.status_code = Empty);
        if let Some(context) = self.input_context.take() {
            span.add_link(context.span().span_context().clone());
        }
        span
    }

    pub fn begin(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    pub fn presented(&mut self, started: Option<Instant>, span: &Span) {
        let Some(started) = started else {
            return;
        };
        let now = Instant::now();
        tracing::event!(target: METRICS_TARGET, Level::INFO, {
            histogram.nc.frame.duration_ms = millis(started, now),
            monotonic_counter.nc.frames = 1_u64,
        });
        if let Some(at) = self.input.take() {
            let latency = millis(at, now);
            span.record("frame.input_ms", latency);
            tracing::event!(target: METRICS_TARGET, Level::INFO, { histogram.nc.input_to_frame_ms = latency });
        }
        if let Some(at) = self.stream.take() {
            let latency = millis(at, now);
            span.record("frame.stream_ms", latency);
            tracing::event!(target: METRICS_TARGET, Level::INFO, { histogram.nc.stream_to_frame_ms = latency });
        }
        if let Some(at) = self.resize.take() {
            let latency = millis(at, now);
            span.record("frame.resize_ms", latency);
            tracing::event!(target: METRICS_TARGET, Level::INFO, { histogram.nc.resize_to_frame_ms = latency });
        }
    }
}

pub fn frame_cpu(span: &Span) -> Option<u64> {
    if span.is_disabled() {
        None
    } else {
        thread_cpu_ns()
    }
}

pub fn record_frame_cpu(span: &Span, started: Option<u64>) {
    if let (Some(started), Some(finished)) = (started, thread_cpu_ns()) {
        span.record("thread.cpu_ns", finished.saturating_sub(started));
    }
}

#[cfg(unix)]
fn thread_cpu_ns() -> Option<u64> {
    let time = nix::time::clock_gettime(nix::time::ClockId::CLOCK_THREAD_CPUTIME_ID).ok()?;
    u64::try_from(time.tv_sec())
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(time.tv_nsec()).ok()?)
}

#[cfg(not(unix))]
const fn thread_cpu_ns() -> Option<u64> {
    None
}

fn millis(from: Instant, to: Instant) -> f64 {
    to.duration_since(from).as_secs_f64() * 1_000.0
}

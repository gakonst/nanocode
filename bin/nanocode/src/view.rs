use std::time::{Duration, Instant};

use nanocode::subagents::AgentId;
use nanocode_observability::TUI_TARGET;
use nanocode_tui::{
    keybindings::{self, Binding},
    ratatui::{
        buffer::Buffer,
        layout::{Constraint, Layout, Position, Rect},
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
    },
    spinner::{self, SPINNER_SLOT, elapsed_label, elapsed_label_deadline},
    terminal::Canvas,
    theme::Theme,
    transcript::Transcript,
};

use crate::{
    app::{App, Overlay, TurnState},
    focus::{Scope, SubagentView},
};

pub const STATUS_VISIBLE: Duration = Duration::from_secs(4);

pub const BINDINGS: [Binding; 36] = [
    Binding {
        keys: "enter",
        action: "send · steer into the running turn",
    },
    Binding {
        keys: "tab",
        action: "queue a follow-up for after the running turn",
    },
    Binding {
        keys: "tab on empty prompt",
        action: "take back the latest queued or steer message to edit",
    },
    Binding {
        keys: "enter on empty prompt",
        action: "steer the latest queued message into the running turn",
    },
    Binding {
        keys: "shift+enter / ctrl+j",
        action: "insert a newline",
    },
    Binding {
        keys: "esc esc",
        action: "interrupt the running turn",
    },
    Binding {
        keys: "f2",
        action: "focus the next subagent; past the last returns to the main thread",
    },
    Binding {
        keys: "alt+u / alt+up",
        action: "take back the latest queued or steer message",
    },
    Binding {
        keys: "@path",
        action: "fuzzy file finder (tab/enter inserts)",
    },
    Binding {
        keys: "!command",
        action: "run a shell command locally",
    },
    Binding {
        keys: "ctrl+v",
        action: "attach an image from the clipboard",
    },
    Binding {
        keys: "up / down",
        action: "prompt history",
    },
    Binding {
        keys: "ctrl+r",
        action: "recent prompts",
    },
    Binding {
        keys: "ctrl+g",
        action: "edit the prompt in $EDITOR",
    },
    Binding {
        keys: "ctrl+z",
        action: "restore the cleared draft",
    },
    Binding {
        keys: "ctrl+d",
        action: "model picker",
    },
    Binding {
        keys: "ctrl+s",
        action: "thinking dial",
    },
    Binding {
        keys: "ctrl+o",
        action: "cycle tools and reasoning: collapsed → beautified → raw",
    },
    Binding {
        keys: "ctrl+t · /agents",
        action: "toggle the tree of active subagents; /agents all lists finished ones too",
    },
    Binding {
        keys: "/agent #id",
        action: "focus a subagent, including finished ones; /agent alone returns",
    },
    Binding {
        keys: "/stop [#id|role]",
        action: "close a subagent and its children (not resumable); alone closes the focused one",
    },
    Binding {
        keys: "pgup / pgdn · wheel",
        action: "scroll the transcript",
    },
    Binding {
        keys: "drag",
        action: "select transcript rows (scrolls at the edges) · release copies them, as raw markdown where whole blocks are selected",
    },
    Binding {
        keys: "click",
        action: "open the link under the pointer",
    },
    Binding {
        keys: "ctrl+home / ctrl+end",
        action: "transcript top / follow",
    },
    Binding {
        keys: "ctrl+l",
        action: "redraw",
    },
    Binding {
        keys: "ctrl+c",
        action: "clear draft · twice to quit",
    },
    Binding {
        keys: "/model /thinking /fast",
        action: "harness, model, thinking, speed",
    },
    Binding {
        keys: "/btw [question]",
        action: "side pane: a forked side thread beside the chat",
    },
    Binding {
        keys: "/btw agent #id · /btw stop #id",
        action: "side pane: focus or close a subagent the side thread spawned; /btw agent alone returns",
    },
    Binding {
        keys: "shift+tab · tab · click",
        action: "switch chat and side pane; tab takes back queued messages first",
    },
    Binding {
        keys: "/collapse /split /close /zoom",
        action: "side pane: merge into its thread, open in a terminal pane, close, full width",
    },
    Binding {
        keys: "/resume · /new",
        action: "switch sessions",
    },
    Binding {
        keys: "/theme /copy /context /skills",
        action: "theme, copy reply, usage, skills",
    },
    Binding {
        keys: "/runner",
        action: "list, connect, listen, disconnect, stop-listening",
    },
    Binding {
        keys: "F1 · /help",
        action: "this help",
    },
];

struct Animation {
    spinner: &'static str,
    cells: Vec<Position>,
    until: Option<Instant>,
}

impl Animation {
    fn line(&mut self, mut line: Line<'static>, area: Rect) -> Line<'static> {
        spinner::animate(&mut line, area, self.spinner, &mut self.cells);
        line
    }

    fn clock(&mut self, started_at: Instant, now: Instant) {
        self.until(Some(elapsed_label_deadline(started_at, now)));
    }

    fn until(&mut self, at: Option<Instant>) {
        if let Some(at) = at {
            self.until = Some(self.until.map_or(at, |until| until.min(at)));
        }
    }

    fn transcript(&mut self, transcript: &Transcript) {
        self.cells.extend_from_slice(transcript.spinner_cells());
        self.until(transcript.clock_deadline());
    }
}

pub fn draw(app: &mut App, frame: &mut Canvas<'_>) {
    let area = frame.area();
    let _span = tracing::info_span!(target: TUI_TARGET, "nc.view", width = area.width, height = area.height).entered();
    let layout = tracing::info_span!(target: TUI_TARGET, "nc.view.layout").entered();
    let now = Instant::now();
    let spinner = app.spinner.frame(now);
    let mut animation = Animation {
        spinner,
        cells: Vec::new(),
        until: None,
    };
    let theme = app.theme.clone();
    let width = area.width;
    let composer_height = app
        .composer
        .height(width.saturating_sub(2))
        .clamp(1, (area.height / 3).max(1))
        + 2;
    let mut queue_lines = app.queue.lines(width, &theme);
    if !app.attachments.is_empty() {
        let names = app
            .attachments
            .iter()
            .map(|path| {
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        queue_lines.push(Line::from(vec![
            Span::styled("⎘ attached: ", Style::default().fg(theme.accent())),
            Span::styled(names, Style::default().fg(theme.text())),
            Span::styled("  · /detach removes", Style::default().fg(theme.muted())),
        ]));
    }
    let tree_lines = if app.show_tree && app.tree.listed() > 0 {
        let focused = match app
            .panes
            .side
            .as_ref()
            .and_then(|side| side.focus)
            .filter(|_| app.panes.active)
        {
            Some(id) => Some(Scope::Side.tree_id(id)),
            None => app.focus.map(|id| Scope::Main.tree_id(id)),
        };
        let selected = focused.and_then(|key| {
            app.tree
                .ordered()
                .iter()
                .position(|(_, _, node)| node.id == key)
        });
        app.tree.lines(&theme, selected)
    } else {
        Vec::new()
    };
    let tree_height = if tree_lines.is_empty() {
        0
    } else {
        u16::try_from(tree_lines.len() + 1).unwrap_or(u16::MAX)
    };
    let panel = app
        .show_panel
        .then(|| app.host.config.clients.render_panel())
        .flatten();
    app.panel_refresh = panel.as_ref().is_some_and(|panel| panel.refresh);
    let panel_lines = panel
        .map(|panel| panel_lines(app, &panel, &theme))
        .unwrap_or_default();
    let panel_height = u16::try_from(panel_lines.len()).unwrap_or(u16::MAX);
    let queue_height = u16::try_from(queue_lines.len()).unwrap_or(u16::MAX);
    let fixed = [
        tree_height,
        panel_height,
        queue_height,
        1,
        composer_height,
        1,
    ]
    .into_iter()
    .try_fold(0_u16, u16::checked_add)
    .filter(|fixed| *fixed < area.height);
    let [
        transcript_area,
        tree_area,
        panel_area,
        queue_area,
        status_area,
        composer_area,
        footer_area,
    ] = match fixed {
        Some(fixed) => stack(
            area,
            [
                area.height - fixed,
                tree_height,
                panel_height,
                queue_height,
                1,
                composer_height,
                1,
            ],
        ),
        None => Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(tree_height),
            Constraint::Length(panel_height),
            Constraint::Length(queue_height),
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .areas(area),
    };
    drop(layout);
    let buffer = frame.buffer_mut();
    transcripts(app, transcript_area, buffer, &theme, &mut animation);
    if tree_height > 0 {
        let finished = match app.tree.finished() {
            0 => String::new(),
            count => format!(" · {count} finished in /agents all"),
        };
        let interrupted = match app.tree.listed() - app.tree.active() {
            0 => String::new(),
            count => format!(" · {count} interrupted"),
        };
        let mut lines = vec![Line::styled(
            format!(
                "agents · {} running{interrupted}{finished} · /stop #id · ctrl+t to hide",
                app.tree.active()
            ),
            Style::default()
                .fg(theme.muted())
                .add_modifier(Modifier::BOLD),
        )];
        lines.extend(
            tree_lines
                .into_iter()
                .zip(tree_area.y + 1..)
                .map(|(line, y)| {
                    animation.line(
                        line,
                        Rect {
                            y,
                            height: 1,
                            ..tree_area
                        }
                        .intersection(tree_area),
                    )
                }),
        );
        Paragraph::new(lines).render(tree_area, buffer);
    }
    Paragraph::new(panel_lines).render(panel_area, buffer);
    Paragraph::new(queue_lines).render(queue_area, buffer);
    let status = status_line(app, now, &mut animation);
    animation
        .line(status, status_area)
        .render(status_area, buffer);
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border()));
    let inner = block.inner(composer_area);
    block.render(composer_area, buffer);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let cursor = app
        .composer
        .render(inner, buffer, &theme, app.overlay.is_none());
    footer(app).render(footer_area, buffer);
    command_popup(app, composer_area, transcript_area, frame);
    finder_popup(app, composer_area, transcript_area, frame);
    if let Some(cursor) = cursor {
        frame.set_cursor_position(cursor);
    }
    match &mut app.overlay {
        Some(Overlay::Picker(_, picker)) => picker.render(area, frame.buffer_mut(), &theme),
        Some(Overlay::Help) => keybindings::render(&BINDINGS, area, frame.buffer_mut(), &theme),
        Some(Overlay::Dial { dial, .. }) => dial.render(area, frame.buffer_mut(), &theme),
        None => frame.selectable(inner),
    }
    let buffer = frame.buffer_mut();
    animation.cells.retain(|position| {
        buffer
            .cell(*position)
            .is_some_and(|cell| cell.symbol() == spinner)
    });
    for position in animation.cells {
        frame.spinner(position);
    }
    app.clock_deadline = animation.until;
}

fn panel_lines(app: &App, panel: &crate::host::Panel, theme: &Theme) -> Vec<Line<'static>> {
    let hide = app
        .host
        .config
        .clients
        .panel_command()
        .map_or_else(String::new, |(command, _)| format!(" · {command} to hide"));
    let mut lines = vec![Line::styled(
        format!("{}{hide}", panel.title),
        Style::default()
            .fg(theme.muted())
            .add_modifier(Modifier::BOLD),
    )];
    lines.extend(panel.aligned().into_iter().map(|row| {
        let mut spans = vec![Span::raw("  ")];
        for (column, cell) in row.into_iter().enumerate() {
            if column > 0 {
                spans.push(Span::raw("  "));
            }
            let color = match column {
                _ if panel.accent.contains(&column) => theme.accent(),
                0 => theme.text(),
                _ => theme.muted(),
            };
            spans.push(Span::styled(cell, Style::default().fg(color)));
        }
        Line::from(spans)
    }));
    lines
}

fn stack<const N: usize>(area: Rect, heights: [u16; N]) -> [Rect; N] {
    let mut y = area.y;
    heights.map(|height| {
        let row = Rect { y, height, ..area };
        y += height;
        row
    })
}

fn transcripts(
    app: &mut App,
    area: Rect,
    buffer: &mut Buffer,
    theme: &Theme,
    animation: &mut Animation,
) {
    let _span = tracing::info_span!(target: TUI_TARGET, "nc.panes.render", side = app.panes.side.is_some(), active_side = app.panes.active, zoomed = app.panes.zoomed).entered();
    let spinner = animation.spinner;
    app.panes.main_area = Rect::default();
    app.panes.side_area = Rect::default();
    if app.panes.side.is_none() {
        let _pane =
            tracing::info_span!(target: TUI_TARGET, "nc.pane.render", pane = "main").entered();
        app.panes.main_area = area;
        let transcript = app.visible_transcript();
        transcript.render(area, buffer, theme, spinner);
        animation.transcript(transcript);
        return;
    }
    let tabs = area.height.min(1);
    let [tabs, body] = stack(area, [tabs, area.height - tabs]);
    animation
        .line(tab_line(app, theme), tabs)
        .render(tabs, buffer);
    if app.panes.zoomed {
        let _pane = tracing::info_span!(target: TUI_TARGET, "nc.pane.render", pane = if app.panes.active { "side" } else { "main" }).entered();
        if app.panes.active {
            app.panes.side_area = body;
        } else {
            app.panes.main_area = body;
        }
        let transcript = app.active_transcript();
        transcript.render(body, buffer, theme, spinner);
        animation.transcript(transcript);
        return;
    }
    let divider = body.x + body.width.saturating_sub(1) / 2;
    app.panes.main_area = Rect {
        width: divider - body.x,
        ..body
    };
    app.panes.side_area = Rect {
        x: divider + 1,
        width: body.right().saturating_sub(divider + 1),
        ..body
    };
    let (main, side) = (app.panes.main_area, app.panes.side_area);
    tracing::info_span!(target: TUI_TARGET, "nc.pane.render", pane = "main").in_scope(|| {
        let transcript = app.pane_transcript(false);
        transcript.render(main, buffer, theme, spinner);
        animation.transcript(transcript);
    });
    tracing::info_span!(target: TUI_TARGET, "nc.pane.render", pane = "side").in_scope(|| {
        let transcript = app.pane_transcript(true);
        transcript.render(side, buffer, theme, spinner);
        animation.transcript(transcript);
    });
    Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme.border()))
        .render(
            Rect {
                x: divider,
                width: 1,
                ..body
            },
            buffer,
        );
}

fn tab_line(app: &App, theme: &Theme) -> Line<'static> {
    let style = |active: bool| {
        if active {
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted())
        }
    };
    let marker = |active: bool| if active { " › " } else { "   " };
    let chat = match app
        .focus
        .and_then(|id| app.views.get(&id).map(|view| (id, view)))
    {
        Some((id, view)) => format!("Chat · #{id} {}", view.role),
        None => "Chat".to_owned(),
    };
    let side = app.panes.side.as_ref();
    let side_style = style(app.panes.active);
    let mut spans = vec![
        Span::styled(
            format!("{}{chat} ", marker(!app.panes.active)),
            style(!app.panes.active),
        ),
        Span::styled(
            format!(
                "{}{}",
                marker(app.panes.active),
                side.map_or_else(String::new, |side| {
                    match side
                        .focus
                        .and_then(|id| side.views.get(&id).map(|view| (id, view)))
                    {
                        Some((id, view)) => format!("BTW · {} · #{id} {}", side.label(), view.role),
                        None => format!("BTW · {} · {}", side.label(), side.kind()),
                    }
                })
            ),
            side_style,
        ),
    ];
    if side.is_some_and(|side| side.busy()) {
        spans.push(Span::styled(" ", side_style));
        spans.push(Span::styled(SPINNER_SLOT, side_style));
    }
    spans.push(Span::styled(" ", side_style));
    spans.push(Span::styled(
        format!(
            "  tab/click: pane · /zoom{} · /collapse · /split · /close · ctrl+c×2 closes",
            if app.panes.zoomed { ": restore" } else { "" }
        ),
        Style::default().fg(theme.muted()),
    ));
    Line::from(spans)
}

pub const FINDER_ROWS: usize = 8;

fn finder_popup(app: &App, composer_area: Rect, transcript_area: Rect, frame: &mut Canvas<'_>) {
    let Some(finder) = &app.finder else {
        return;
    };
    if app.overlay.is_some() {
        return;
    }
    let _span = tracing::info_span!(target: TUI_TARGET, "nc.finder.render", files = finder.finder.len(), matches = finder.finder.matches()).entered();
    let rows = FINDER_ROWS.min(usize::from(transcript_area.height.saturating_sub(1)));
    let lines = finder.finder.lines(rows, &app.theme);
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let popup = Rect {
        x: composer_area.x,
        y: composer_area.y.saturating_sub(height + 1),
        width: composer_area.width,
        height: height + 1,
    };
    let buffer = frame.buffer_mut();
    Clear.render(popup, buffer);
    let mut all = vec![Line::styled(
        format!(
            "  files matching @{} · {} of {} · tab/enter insert · esc close",
            finder.finder.query(),
            finder.finder.matches(),
            finder.finder.len()
        ),
        Style::default().fg(app.theme.muted()),
    )];
    all.extend(lines);
    Paragraph::new(all).render(popup, buffer);
}

fn command_popup(app: &App, composer_area: Rect, transcript_area: Rect, frame: &mut Canvas<'_>) {
    let text = app.composer.text();
    if app.overlay.is_some() || !text.starts_with('/') || text.contains('\n') {
        return;
    }
    let theme = &app.theme;
    let matches = if text.contains(' ') {
        app.agent_argument_candidates(text).unwrap_or_default()
    } else {
        crate::commands::matching(text, app.host.config.clients.panel_command())
            .map(|(command, description)| (command.to_owned(), description.to_owned()))
            .collect::<Vec<_>>()
    };
    if matches.is_empty() {
        return;
    }
    let _span =
        tracing::info_span!(target: TUI_TARGET, "nc.commands.render", matches = matches.len())
            .entered();
    let height = u16::try_from(matches.len())
        .unwrap_or(u16::MAX)
        .min(transcript_area.height);
    let popup = Rect {
        x: composer_area.x,
        y: composer_area.y.saturating_sub(height + 1),
        width: composer_area.width,
        height: height + 1,
    };
    let lines = matches
        .into_iter()
        .enumerate()
        .map(|(index, (command, description))| {
            let style = if index == 0 {
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text())
            };
            Line::from(vec![
                Span::styled(format!("  {command:<12}"), style),
                Span::styled(description, Style::default().fg(theme.muted())),
            ])
        })
        .collect::<Vec<_>>();
    let buffer = frame.buffer_mut();
    Clear.render(popup, buffer);
    let mut all = vec![Line::styled(
        "  tab to complete",
        Style::default().fg(theme.muted()),
    )];
    all.extend(lines);
    Paragraph::new(all).render(popup, buffer);
}

fn spinning(style: Style, rest: impl IntoIterator<Item = Span<'static>>) -> Line<'static> {
    let mut spans = vec![
        Span::styled(" ", style),
        Span::styled(SPINNER_SLOT, style),
        Span::styled(" ", style),
    ];
    spans.extend(rest);
    Line::from(spans)
}

fn focus_line(app: &App, scope: Scope, id: AgentId, view: &SubagentView) -> Line<'static> {
    let theme = &app.theme;
    let key = scope.tree_id(id);
    let state = app
        .tree
        .nodes()
        .iter()
        .find(|node| node.id == key)
        .map(|node| node.status.label())
        .unwrap_or_default();
    let prefix = scope.prefix();
    Line::from(vec![
        Span::styled(" ▸ ", Style::default().fg(theme.accent())),
        Span::styled(
            format!("{prefix}#{id} {}", view.role),
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{}{}{state}",
                view.model
                    .as_deref()
                    .map(|model| format!(" · {model}"))
                    .unwrap_or_default(),
                if state.is_empty() { "" } else { " · " }
            ),
            Style::default().fg(theme.muted()),
        ),
        Span::styled(
            format!("  enter messages it · esc esc interrupts · /{prefix}agent returns"),
            Style::default().fg(theme.muted()),
        ),
    ])
}

fn status_line(app: &App, now: Instant, animation: &mut Animation) -> Line<'static> {
    let _span = tracing::info_span!(target: TUI_TARGET, "nc.status.render").entered();
    let theme = &app.theme;
    if let Some((text, at)) = &app.status
        && now.duration_since(*at) < STATUS_VISIBLE
    {
        return Line::styled(
            format!(" {text}"),
            Style::default().fg(theme.thinking_high()),
        );
    }
    if app.panes.side_active()
        && let Some(side) = &app.panes.side
    {
        if let Some((id, view)) = side
            .focus
            .and_then(|id| side.views.get(&id).map(|view| (id, view)))
        {
            return focus_line(app, Scope::Side, id, view);
        }
        let state = match side.since() {
            Some(since) if side.busy() => {
                animation.clock(since, now);
                format!(
                    " · answering {} · esc esc cancels",
                    elapsed_label(now.duration_since(since))
                )
            }
            _ => String::new(),
        };
        let waiting = match side.waiting() {
            0 => String::new(),
            count => format!(" · {count} queued"),
        };
        let rest = [
            Span::styled(
                format!("btw · {}", side.label()),
                Style::default()
                    .fg(theme.text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " · {}{state}{waiting}  enter asks · tab returns to the chat",
                    side.kind()
                ),
                Style::default().fg(theme.muted()),
            ),
        ];
        let style = Style::default().fg(theme.accent());
        return if side.busy() {
            spinning(style, rest)
        } else {
            let mut spans = vec![Span::styled(" ▸ ", style)];
            spans.extend(rest);
            Line::from(spans)
        };
    }
    if let Some(id) = app.focus
        && let Some(view) = app.views.get(&id)
    {
        return focus_line(app, Scope::Main, id, view);
    }
    if app.interrupting && !matches!(app.turn, TurnState::Idle) {
        return spinning(
            Style::default().fg(theme.thinking_max()),
            [Span::styled(
                "interrupting…",
                Style::default().fg(theme.text()),
            )],
        );
    }
    match &app.turn {
        TurnState::Running { since, .. }
        | TurnState::Starting(since)
        | TurnState::External(since) => {
            animation.clock(*since, now);
            spinning(
                Style::default().fg(theme.accent()),
                [
                    Span::styled(
                        "Working",
                        Style::default()
                            .fg(theme.text())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            " ({} · {} · enter steers · tab queues)",
                            elapsed_label(now.duration_since(*since)),
                            if matches!(app.turn, TurnState::External(_)) {
                                "woken by a finished subagent · esc esc to interrupt"
                            } else {
                                "esc esc to interrupt"
                            }
                        ),
                        Style::default().fg(theme.muted()),
                    ),
                ],
            )
        }
        TurnState::Idle if app.starting => spinning(
            Style::default().fg(theme.accent()),
            [Span::styled(
                "connecting…",
                Style::default().fg(theme.muted()),
            )],
        ),
        TurnState::Idle => Line::styled(
            " F1 for keys · / for commands",
            Style::default().fg(theme.muted()),
        ),
    }
}

fn footer(app: &App) -> Line<'static> {
    let _span = tracing::info_span!(target: TUI_TARGET, "nc.footer.render").entered();
    let theme = &app.theme;
    let mut spans = vec![
        Span::styled(
            format!(" {} ", crate::app::family_label(&app.model.family())),
            Style::default()
                .fg(theme.code_background())
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", app.model),
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · {}", app.thinking),
            Style::default().fg(theme.muted()),
        ),
    ];
    if app.tier != nanocode::ServiceTier::Standard {
        spans.push(Span::styled(
            format!(" · {}", app.tier.as_str()),
            Style::default().fg(theme.thinking_high()),
        ));
    }
    if let Some(gateway) = app
        .gateway()
        .map(|gateway| app.host.config.routes.gateway(gateway))
        .filter(|gateway| !gateway.login)
    {
        spans.push(Span::styled(
            format!(" · via {}", gateway.name),
            Style::default().fg(theme.muted()),
        ));
    }
    if let Some(status) = app.host.config.clients.describe() {
        spans.push(Span::styled(
            format!(" · {status}"),
            Style::default().fg(theme.accent()),
        ));
    }
    if let Some((model, thinking, _)) = &app.pending_switch {
        spans.push(Span::styled(
            format!(" → {model} {thinking} after turn"),
            Style::default().fg(theme.thinking_medium()),
        ));
    }
    if let Some(session) = app.session_id() {
        let short = session
            .get(session.len().saturating_sub(8)..)
            .unwrap_or(&session)
            .to_owned();
        spans.push(Span::styled(
            format!(" · {short}"),
            Style::default().fg(theme.muted()),
        ));
    }
    if app.host.config.store.is_none() {
        spans.push(Span::styled(
            " · ephemeral",
            Style::default().fg(theme.muted()),
        ));
    }
    let runner_status = app.runner_status();
    if runner_status != "local tools" {
        spans.push(Span::styled(
            format!(" · {runner_status}"),
            Style::default().fg(theme.muted()),
        ));
    }
    spans.push(Span::styled(
        format!(
            " · {}",
            nanocode_tui::format::shorten_home(&app.host.config.workspace)
        ),
        Style::default().fg(theme.muted()),
    ));
    Line::from(spans)
}

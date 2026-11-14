use std::time::{SystemTime, UNIX_EPOCH};

use nanocode::durability::{SessionSummary, TranscriptItem};
use nanocode_tui::{
    picker::{Picker, PickerItem},
    transcript::{CellKind, ToolStatus, Transcript},
};

pub const PAGE: usize = 100;

pub fn age(updated_at_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    let seconds = now.saturating_sub(updated_at_ms) / 1000;
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

pub fn title(session: &SessionSummary) -> String {
    session
        .record
        .title
        .clone()
        .or_else(|| session.preview.clone())
        .map(|text| text.lines().next().unwrap_or_default().to_owned())
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "(no prompt yet)".into())
}

pub fn model(record: &nanocode::durability::SessionRecord) -> String {
    match (
        crate::host::Route::of(record.provider.as_deref()),
        &record.provider,
    ) {
        (Ok(route), _) => route.label(&record.model),
        (Err(_), Some(provider)) => format!("{provider}/{}", record.model),
        (Err(_), None) => record.model.to_string(),
    }
}

pub fn picker(sessions: &[SessionSummary]) -> Picker {
    let items = sessions
        .iter()
        .map(|session| {
            PickerItem::choice(title(session)).detail(format!(
                "{} · {} · {}",
                age(session.record.updated_at_ms),
                model(&session.record),
                session.record.session_id
            ))
        })
        .collect();
    Picker::new("Resume session", items)
        .filterable()
        .footer("type to filter · enter resume · esc cancel")
}

pub fn session_at<'a>(
    sessions: &'a [SessionSummary],
    picker: &Picker,
    index: usize,
) -> Option<&'a SessionSummary> {
    let item = picker.items().get(index)?;
    sessions
        .iter()
        .find(|session| item.detail.ends_with(&session.record.session_id))
}

pub fn replay(transcript: &mut Transcript, history: Vec<TranscriptItem>) {
    let _span = nanocode_trace::info_span!(target: "nanocode", "nc.session.replay", items = history.len())
        .entered();
    let mut mapper = crate::events::EventMapper::default();
    for item in history {
        match item {
            TranscriptItem::User(text) if text.trim_start().starts_with("<turn_aborted>") => {
                transcript.push(CellKind::Notice(
                    nanocode_tui::transcript::NoticeLevel::Warning,
                    "turn interrupted".to_owned(),
                ));
            }
            TranscriptItem::User(text) => {
                transcript.push(CellKind::User(
                    crate::side::prompt_display(&text).to_owned(),
                ));
            }
            TranscriptItem::Assistant(text) => {
                transcript.push(CellKind::Assistant(text));
            }
            TranscriptItem::Reasoning(text) => {
                if !text.trim().is_empty() {
                    transcript.push(CellKind::Reasoning(text));
                }
            }
            TranscriptItem::Tool {
                call_id,
                name,
                arguments,
                parent_call_id,
            } => mapper.replay_tool_call(
                transcript,
                &call_id,
                &name,
                &arguments,
                parent_call_id.as_deref(),
            ),
            TranscriptItem::ToolResult {
                call_id,
                output,
                outcome,
            } => mapper.replay_tool_result(
                transcript,
                &call_id,
                &output,
                matches!(outcome, nanocode::agent::session::ToolOutcome::Failed),
            ),
        }
    }
    transcript.finish_running_tools(ToolStatus::Completed);
}

pub const fn user_facing(origin: nanocode::Origin) -> bool {
    matches!(
        origin,
        nanocode::Origin::Root | nanocode::Origin::Fork | nanocode::Origin::Branch
    )
}

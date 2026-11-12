use std::{
    borrow::Cow,
    time::{Duration, Instant},
};

use crate::ui::{
    layout::{Position, Rect},
    text::Line,
};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub const SPINNER_SLOT: &str = "\u{0}";

pub const LABEL_TICK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug)]
pub struct Spinner {
    started_at: Instant,
    interval: Duration,
}

impl Spinner {
    #[must_use]
    pub const fn new(started_at: Instant, interval: Duration) -> Self {
        Self {
            started_at,
            interval,
        }
    }

    #[must_use]
    pub const fn started_at(&self) -> Instant {
        self.started_at
    }

    #[must_use]
    pub fn frame(&self, now: Instant) -> &'static str {
        FRAMES[self.tick(now) % FRAMES.len()]
    }

    #[must_use]
    pub fn next_deadline(&self, now: Instant) -> Instant {
        let next = u32::try_from(self.tick(now) + 1).unwrap_or(u32::MAX);
        self.started_at + self.interval * next
    }

    fn tick(&self, now: Instant) -> usize {
        let elapsed = now.saturating_duration_since(self.started_at);
        usize::try_from(elapsed.as_nanos() / self.interval.as_nanos().max(1)).unwrap_or(usize::MAX)
    }
}

pub fn animate(line: &mut Line<'_>, area: Rect, spinner: &'static str, cells: &mut Vec<Position>) {
    let mut column = 0;
    for span in &mut line.spans {
        if span.content == SPINNER_SLOT {
            span.content = Cow::Borrowed(spinner);
            if area.height > 0
                && let Ok(column) = u16::try_from(column)
                && column < area.width
            {
                cells.push(Position::new(area.x + column, area.y));
            }
        }
        column += span.width();
    }
}

#[must_use]
pub fn elapsed_label_deadline(started_at: Instant, now: Instant) -> Instant {
    let ticks = now.saturating_duration_since(started_at).as_nanos() / LABEL_TICK.as_nanos() + 1;
    started_at + LABEL_TICK * u32::try_from(ticks).unwrap_or(u32::MAX)
}

#[must_use]
pub fn elapsed_label(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

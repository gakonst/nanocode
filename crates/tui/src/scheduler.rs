use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RenderScheduler {
    frame_interval: Duration,
    last_streamed: Option<Instant>,
    frame: Option<Frame>,
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    deadline: Instant,
    streaming: bool,
}

impl RenderScheduler {
    #[must_use]
    pub const fn new(frame_interval: Duration, now: Instant) -> Self {
        Self {
            frame_interval,
            last_streamed: None,
            frame: Some(Frame {
                deadline: now,
                streaming: false,
            }),
        }
    }

    pub fn request_streaming(&mut self, now: Instant) {
        let next_frame = self
            .last_streamed
            .map_or(now, |streamed| streamed + self.frame_interval);
        self.request(next_frame.max(now), true);
    }

    pub fn request_immediate(&mut self, now: Instant) {
        self.request(now, false);
    }

    pub fn request_at(&mut self, at: Instant) {
        self.request(at, false);
    }

    fn request(&mut self, at: Instant, streaming: bool) {
        self.frame = Some(match self.frame {
            Some(frame) => Frame {
                deadline: frame.deadline.min(at),
                streaming: frame.streaming || streaming,
            },
            None => Frame {
                deadline: at,
                streaming,
            },
        });
    }

    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        match self.frame {
            Some(frame) => Some(frame.deadline),
            None => None,
        }
    }

    #[must_use]
    pub fn next_frame_at(&self, now: Instant) -> Instant {
        self.last_streamed
            .map(|streamed| streamed + self.frame_interval)
            .filter(|next| *next > now)
            .unwrap_or(now + self.frame_interval)
    }

    #[must_use]
    pub fn is_due(&self, now: Instant) -> bool {
        self.frame.is_some_and(|frame| frame.deadline <= now)
    }

    pub const fn presented(&mut self, now: Instant) {
        if let Some(Frame {
            streaming: true, ..
        }) = self.frame.take()
        {
            self.last_streamed = Some(now);
        }
    }
}

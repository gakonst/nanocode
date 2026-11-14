use std::fmt::Write as _;

use nanocode_trace::{
    Event, Level, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

struct Printer {
    level: Level,
}

impl Subscriber for Printer {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= &self.level
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        eprintln!(
            "LOG {} {}{}",
            event.metadata().level(),
            event.metadata().target(),
            fields.0
        );
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

pub fn install() {
    let Some(level) = std::env::var("NANOCODE_SIM_LOG")
        .ok()
        .and_then(|level| level.parse::<Level>().ok())
    else {
        return;
    };
    let _ = nanocode_trace::subscriber::set_global_default(Printer { level });
}

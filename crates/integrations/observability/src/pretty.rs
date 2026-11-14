use std::fmt::{self, Write as _};

use nanocode_trace::{
    Event,
    field::{Field, Visit},
};

use crate::{
    format::{
        ErrorSourceList, EscapeGuard, FmtLevel, SpanFields, current_scope, event_meta,
        level_style, write_timestamp,
    },
    style::Style,
    subscriber::Context,
};

pub(crate) fn format_event(
    context: &Context<'_, SpanFields>,
    writer: &mut String,
    event: &Event<'_>,
    ansi: bool,
) -> fmt::Result {
    let meta = event_meta(event);
    writer.push_str("  ");
    if ansi {
        let dimmed = Style::new().dimmed();
        write!(writer, "{}", dimmed.prefix())?;
        write_timestamp(writer)?;
        write!(writer, "{} ", dimmed.suffix())?;
    } else {
        write_timestamp(writer)?;
        writer.push(' ');
    }
    let style = if ansi {
        level_style(meta.level)
    } else {
        Style::new()
    };
    write!(writer, "{} ", FmtLevel::new(meta.level, ansi))?;
    let target_style = if ansi { style.bold() } else { style };
    write!(
        writer,
        "{}{}{}:",
        target_style.prefix(),
        meta.target,
        target_style.infix(style)
    )?;
    writer.push(' ');
    let mut visitor = PrettyVisitor::new(writer, true, ansi).with_style(style);
    event.record(&mut visitor);
    visitor.finish()?;
    writer.push('\n');
    let dimmed = if ansi {
        Style::new().dimmed().italic()
    } else {
        Style::new()
    };
    if let Some(file) = meta.file {
        write!(writer, "    {} {}", dimmed.paint("at"), file)?;
        if let Some(line) = meta.line {
            write!(writer, ":{line}")?;
        }
        writer.push('\n');
    }
    let bold = if ansi { Style::new().bold() } else { Style::new() };
    for span in current_scope(context, event).into_iter().flatten() {
        let metadata = span.metadata();
        write!(
            writer,
            "    {} {}::{}",
            dimmed.paint("in"),
            metadata.target(),
            bold.paint(metadata.name())
        )?;
        let data = span.data().expect("a visible span has its layer's data");
        data.text(|text| {
            let text = text.expect("Unable to find FormattedFields in extensions; this is a bug");
            if text.is_empty() {
                Ok(())
            } else {
                write!(writer, " {} {}", dimmed.paint("with"), text)
            }
        })?;
        writer.push('\n');
    }
    writer.push('\n');
    Ok(())
}

pub(crate) struct PrettyVisitor<'a> {
    writer: &'a mut String,
    is_empty: bool,
    ansi: bool,
    style: Style,
    result: fmt::Result,
}

impl<'a> PrettyVisitor<'a> {
    pub(crate) const fn new(writer: &'a mut String, is_empty: bool, ansi: bool) -> Self {
        Self {
            writer,
            is_empty,
            ansi,
            style: Style::new(),
            result: Ok(()),
        }
    }

    const fn with_style(self, style: Style) -> Self {
        Self { style, ..self }
    }

    fn write_padded(&mut self, value: &dyn fmt::Debug) {
        let padding = if self.is_empty {
            self.is_empty = false;
            ""
        } else {
            ", "
        };
        self.result = write!(self.writer, "{padding}{value:?}");
    }

    const fn bold(&self) -> Style {
        if self.ansi {
            self.style.bold()
        } else {
            Style::new()
        }
    }

    pub(crate) fn finish(self) -> fmt::Result {
        write!(self.writer, "{}", self.style.suffix())?;
        self.result
    }
}

impl Visit for PrettyVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if self.result.is_err() {
            return;
        }
        if field.name() == "message" {
            self.record_debug(field, &format_args!("{value}"));
        } else {
            self.record_debug(field, &value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        if let Some(source) = value.source() {
            let bold = self.bold();
            let style = self.style;
            self.record_debug(
                field,
                &format_args!(
                    "{}, {}{}.sources{}: {}",
                    EscapeGuard(format_args!("{value}")),
                    bold.prefix(),
                    field,
                    bold.infix(style),
                    ErrorSourceList(source),
                ),
            );
        } else {
            self.record_debug(field, &EscapeGuard(format_args!("{value}")));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.result.is_err() {
            return;
        }
        let bold = self.bold();
        let style = self.style;
        match field.name() {
            "message" => self.write_padded(&format_args!(
                "{}{:?}",
                style.prefix(),
                EscapeGuard(value)
            )),
            name if name.starts_with("r#") => self.write_padded(&format_args!(
                "{}{}{}: {:?}",
                bold.prefix(),
                &name[2..],
                bold.infix(style),
                value
            )),
            name => self.write_padded(&format_args!(
                "{}{}{}: {:?}",
                bold.prefix(),
                name,
                bold.infix(style),
                value
            )),
        }
    }
}


use std::io;
use std::path::Path;

use super::color::ColorSpecs;
use super::hyperlink::{self, HyperlinkConfig, InterpolatorStatus};
use super::util::PrinterPath;
use crate::ansi::WriteColor;

#[derive(Clone, Debug)]
struct Config {
    colors: ColorSpecs,
    hyperlink: HyperlinkConfig,
    separator: Option<u8>,
    terminator: u8,
}

#[derive(Clone, Debug)]
pub(crate) struct PathPrinterBuilder {
    config: Config,
}

impl PathPrinterBuilder {
    pub(crate) fn new() -> PathPrinterBuilder {
        PathPrinterBuilder {
            config: Config {
                colors: ColorSpecs::default(),
                hyperlink: HyperlinkConfig::default(),
                separator: None,
                terminator: b'\n',
            },
        }
    }

    pub(crate) fn build<W: WriteColor>(&self, writer: W) -> PathPrinter<W> {
        let interpolator = hyperlink::Interpolator::new(&self.config.hyperlink);
        PathPrinter {
            config: self.config.clone(),
            writer,
            interpolator,
        }
    }

    pub(crate) fn color_specs(&mut self, specs: ColorSpecs) -> &mut PathPrinterBuilder {
        self.config.colors = specs;
        self
    }

    pub(crate) fn hyperlink(&mut self, config: HyperlinkConfig) -> &mut PathPrinterBuilder {
        self.config.hyperlink = config;
        self
    }

    pub(crate) fn separator(&mut self, separator: Option<u8>) -> &mut PathPrinterBuilder {
        self.config.separator = separator;
        self
    }

    pub(crate) fn terminator(&mut self, terminator: u8) -> &mut PathPrinterBuilder {
        self.config.terminator = terminator;
        self
    }
}

#[derive(Debug)]
pub(crate) struct PathPrinter<W> {
    config: Config,
    writer: W,
    interpolator: hyperlink::Interpolator,
}

impl<W: WriteColor> PathPrinter<W> {
    pub(crate) fn write(&mut self, path: &Path) -> io::Result<()> {
        let printer_path = PrinterPath::new(path).with_separator(self.config.separator);
        if !self.writer.supports_color() {
            self.writer.write_all(printer_path.as_bytes())?;
        } else {
            let status = match printer_path.as_hyperlink() {
                None => InterpolatorStatus::Inactive,
                Some(hyperlink_path) => self
                    .interpolator
                    .begin(&hyperlink::Values::new(hyperlink_path), &mut self.writer)?,
            };
            self.writer.set_color(self.config.colors.path())?;
            self.writer.write_all(printer_path.as_bytes())?;
            self.writer.reset()?;
            self.interpolator.finish(status, &mut self.writer)?;
        }
        self.writer.write_all(&[self.config.terminator])
    }
}

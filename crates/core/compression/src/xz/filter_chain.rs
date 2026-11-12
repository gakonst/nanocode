use super::coder::Coder;
use super::options::Filter;
use super::stream::Error;
use super::{delta, lz_decoder, lz_encoder, simple};

pub(crate) const FILTERS_MAX: usize = 4;
const SIMPLE_FILTER_MEMUSAGE_BYTES: u64 = 1024;
pub(crate) const MEMUSAGE_BASE_BYTES: u64 = 1 << 15;

pub(crate) fn validate(filters: &[Filter]) -> Result<(), Error> {
    let Some((last, others)) = filters.split_last() else {
        return Err(Error::Program);
    };
    if others.iter().any(Filter::is_lzma) {
        return Err(Error::Options);
    }
    if filters.len() > FILTERS_MAX || !last.is_lzma() {
        return Err(Error::Options);
    }
    Ok(())
}

fn validate_options(filter: &Filter) -> Result<(), Error> {
    match *filter {
        Filter::Delta { distance } => delta::validate(distance),
        Filter::Bcj {
            architecture,
            start_offset,
        } => simple::validate(architecture, start_offset),
        Filter::Lzma1(_) | Filter::Lzma2(_) => Ok(()),
    }
}

fn lzma_encoder(filter: &Filter, next: Option<Coder>) -> Result<Coder, Error> {
    match filter {
        Filter::Lzma1(options) => lz_encoder::lzma1_encoder(options, next),
        Filter::Lzma2(options) => lz_encoder::lzma2_encoder(options, next),
        Filter::Delta { .. } | Filter::Bcj { .. } => unreachable!("validated chains end with LZMA"),
    }
}

pub(crate) fn raw_decoder(filters: &[Filter]) -> Result<Coder, Error> {
    validate(filters)?;
    let (last, others) = filters
        .split_last()
        .expect("a validated chain is not empty");
    for filter in others {
        validate_options(filter)?;
    }
    let mut coder = match last {
        Filter::Lzma1(options) => lz_decoder::lzma1_decoder(options, None, true)?,
        Filter::Lzma2(options) => lz_decoder::lzma2_decoder(options)?,
        Filter::Delta { .. } | Filter::Bcj { .. } => unreachable!("validated chains end with LZMA"),
    };
    for filter in others.iter().rev() {
        coder = match *filter {
            Filter::Delta { distance } => delta::decoder(distance, coder)?,
            Filter::Bcj {
                architecture,
                start_offset,
            } => simple::decoder(architecture, start_offset, coder)?,
            Filter::Lzma1(_) | Filter::Lzma2(_) => unreachable!("validated chains end with LZMA"),
        };
    }
    Ok(coder)
}

pub(crate) fn raw_encoder(filters: &[Filter]) -> Result<Coder, Error> {
    validate(filters)?;
    let (last, others) = filters
        .split_last()
        .expect("a validated chain is not empty");
    if let Some(error) = others
        .iter()
        .rev()
        .find_map(|filter| validate_options(filter).err())
    {
        lzma_encoder(last, None)?;
        return Err(error);
    }
    let mut next: Option<Coder> = None;
    for filter in others {
        next = Some(match *filter {
            Filter::Delta { distance } => delta::encoder(distance, next.take())?,
            Filter::Bcj {
                architecture,
                start_offset,
            } => simple::encoder(architecture, start_offset, next.take())?,
            Filter::Lzma1(_) | Filter::Lzma2(_) => unreachable!("validated chains end with LZMA"),
        });
    }
    lzma_encoder(last, next)
}

pub(crate) fn raw_decoder_memusage(filters: &[Filter]) -> Option<u64> {
    validate(filters).ok()?;
    let mut total = MEMUSAGE_BASE_BYTES;
    for filter in filters {
        total += match filter {
            Filter::Lzma1(options) => lz_decoder::lzma1_decoder_memusage(options)?,
            Filter::Lzma2(options) => lz_decoder::lzma2_decoder_memusage(options),
            Filter::Delta { .. } => delta::MEMUSAGE_BYTES,
            Filter::Bcj { .. } => SIMPLE_FILTER_MEMUSAGE_BYTES,
        };
    }
    Some(total)
}

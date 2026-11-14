// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

#[cfg(test)]
use core::mem;

use crate::Error;

/// The library's external and internal trait for all
/// random consumption.
pub trait RandomSource {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error>;
}

/// Random generation from the system entropy source via
/// the `getrandom` crate.
pub(crate) struct SystemRandom;

impl RandomSource for SystemRandom {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        getrandom::fill(out).map_err(|_| Error::RngFailed)
    }
}

/// Random generation from a slice.
///
/// Returns an error once exhausted.  Intended only for testing.
#[cfg(test)]
pub(crate) struct SliceRandomSource<'a>(pub &'a [u8]);

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl RandomSource for SliceRandomSource<'_> {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        if out.len() > self.0.len() {
            return Err(Error::RngFailed);
        }

        let (chunk, rest) = self.0.split_at(out.len());
        self.0 = rest;
        out.copy_from_slice(chunk);
        Ok(())
    }
}

/// Random generation from two subsources.
///
/// The first subsource is used first, until it returns an error.
/// Then the second is used for all subsequent requests.
///
/// Intended only for testing.
#[cfg(test)]
pub(crate) enum ChainRandomSource<'a> {
    First(&'a mut dyn RandomSource, &'a mut dyn RandomSource),
    Rest(&'a mut dyn RandomSource),
    Dead,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl RandomSource for ChainRandomSource<'_> {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        let taken = mem::replace(self, Self::Dead);
        *self = match taken {
            ChainRandomSource::First(source, rest) => match source.fill(out) {
                Ok(()) => Self::First(source, rest),
                Err(_) => {
                    rest.fill(out)?;
                    ChainRandomSource::Rest(rest)
                }
            },
            ChainRandomSource::Rest(source) => {
                source.fill(out)?;
                Self::Rest(source)
            }
            ChainRandomSource::Dead => return Err(Error::RngFailed),
        };
        Ok(())
    }
}

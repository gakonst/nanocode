// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0


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
        nanocode_platform::fill_random(out).map_err(|_| Error::RngFailed)
    }
}





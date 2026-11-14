// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use super::{enter_cpu_state, leave_cpu_state, verify_cpu_features};

/// One of these should be made at library entry points: every `pub` function.
///
/// Trivial conversions, accessors (ie, functions which do no actual computation)
/// need not.
pub(crate) struct Entry {
    secret: bool,
    cpu_state: u32,
}

impl Entry {
    /// Must be called at top-level crate entry points for public functions.
    ///
    /// Public functions have no secret data in their arguments or return values.
    #[must_use]
    pub(crate) fn new_public() -> Self {
        verify_cpu_features();

        Self {
            secret: false,
            cpu_state: 0,
        }
    }

    /// Must be called at top-level crate entry points for secret functions.
    ///
    /// Secret functions have secret data in their arguments or return values
    /// (directly, or transitively).
    #[must_use]
    pub(crate) fn new_secret() -> Self {
        verify_cpu_features();
        let cpu_state = enter_cpu_state();

        Self {
            secret: true,
            cpu_state,
        }
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        if self.secret {
            leave_cpu_state(self.cpu_state);
        }
    }
}

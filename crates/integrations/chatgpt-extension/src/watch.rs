//! Keeping a host's ChatGPT bridge in order while its extension link lives:
//! repairing the registrations ChatGPT updates overwrite, choosing which
//! browser's ChatGPT extension to use, and telling the link why no browser is
//! connected.

use crate::{Bridge, Choice, ExtensionLink, cached_default_browser};
use std::{collections::HashSet, path::PathBuf, sync::Weak, time::Duration};
use tokio::{task::spawn_blocking, time::sleep};

/// What a [`Watch`] noticed, for the host to log.
#[derive(Clone, Copy, Debug)]
pub enum Noticed<'a> {
    /// The hand now uses the ChatGPT extension in this browser.
    Proxied(&'static str),
    /// It stopped using it there.
    Released(&'static str),
    /// It cannot use it there, and why; each failure once.
    Failed(&'static str, &'a str),
    /// Why a browser has not started the connector or what stopped it; each
    /// reason once.
    Waiting(&'a str),
    /// Registrations repaired, more than none.
    Repaired(usize),
    /// Repairing failed; once until the error changes.
    Unrepaired(&'a str),
    /// A round failed on its own (its blocking work panicked).
    Broken(&'a str),
}

/// Keeps a [`Bridge`] in order for as long as its [`ExtensionLink`] lives: every
/// round (3 s) it repairs the registrations (`Bridge::reconcile_all`), and every
/// tenth, starting with the first, it chooses which browser's ChatGPT extension
/// to use (`Bridge::choose`, on macOS while the bridge's state directory
/// exists), reads why browsers have not started the connector
/// (`Bridge::not_started`), and sets the link's unavailable reason.
#[derive(Clone, Debug)]
pub struct Watch {
    bridge: Bridge,
    home: PathBuf,
    preferred: Vec<String>,
    every: Duration,
    choosing: u64,
}

impl Watch {
    /// `bridge` with its launcher set (choosing installs it); `home` holds the
    /// browsers' profiles; `preferred` are the host's own extensions, whose being
    /// on anywhere hands a chosen browser back.
    pub fn new(bridge: Bridge, home: impl Into<PathBuf>, preferred: &[&str]) -> Self {
        Self {
            bridge,
            home: home.into(),
            preferred: preferred.iter().map(|id| (*id).to_owned()).collect(),
            every: Duration::from_secs(3),
            choosing: 10,
        }
    }

    /// How long between rounds.
    #[must_use]
    pub const fn every(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }

    /// Every how many rounds it chooses (at least 1).
    #[must_use]
    pub const fn choosing(mut self, rounds: u64) -> Self {
        self.choosing = if rounds == 0 { 1 } else { rounds };
        self
    }

    /// Runs until `link` is dropped. `unavailable(failed, waiting)` words the
    /// link's reason from the failed choices (`browser: error`) and the
    /// not-started reasons, or clears it with `None`; `noticed` hears what
    /// happened, for the host's log.
    pub async fn run(
        self,
        link: Weak<ExtensionLink>,
        mut unavailable: impl FnMut(&[String], &[String]) -> Option<String> + Send,
        mut noticed: impl FnMut(Noticed<'_>) + Send,
    ) {
        let (mut last, mut seen, mut round) = (None::<String>, HashSet::new(), 0u64);
        while let Some(alive) = link.upgrade() {
            let choose = round.is_multiple_of(self.choosing);
            round = round.wrapping_add(1);
            let (bridge, home, preferred) = (
                self.bridge.clone(),
                self.home.clone(),
                self.preferred.clone(),
            );
            let checked = spawn_blocking(move || {
                let choices = if choose && cfg!(target_os = "macos") && bridge.state().is_dir() {
                    let preferred: Vec<&str> = preferred.iter().map(String::as_str).collect();
                    bridge.choose(&preferred, cached_default_browser(&home))
                } else {
                    Vec::new()
                };
                let waiting = if choose {
                    bridge.not_started()
                } else {
                    Vec::new()
                };
                let repaired = bridge.reconcile_all().map_err(|error| error.to_string());
                (choices, waiting, repaired)
            });
            let (choices, waiting, repaired) = match checked.await {
                Ok(checked) => checked,
                Err(error) => {
                    noticed(Noticed::Broken(&error.to_string()));
                    drop(alive);
                    sleep(self.every).await;
                    continue;
                }
            };
            if choose {
                let failed: Vec<String> = choices
                    .iter()
                    .filter_map(|choice| match choice {
                        Choice::Failed(name, error) => Some(format!("{name}: {error}")),
                        _ => None,
                    })
                    .collect();
                for reason in &waiting {
                    if seen.insert(reason.clone()) {
                        noticed(Noticed::Waiting(reason));
                    }
                }
                alive.set_unavailable(unavailable(&failed, &waiting));
            }
            for choice in &choices {
                match choice {
                    Choice::Proxied(name) => noticed(Noticed::Proxied(name)),
                    Choice::Released(name) => noticed(Noticed::Released(name)),
                    Choice::Failed(name, error) if seen.insert(format!("{name}: {error}")) => {
                        noticed(Noticed::Failed(name, error));
                    }
                    Choice::Failed(..) => {}
                }
            }
            match repaired {
                Ok(repaired) => {
                    last = None;
                    if repaired > 0 {
                        noticed(Noticed::Repaired(repaired));
                    }
                }
                Err(error) => {
                    if last.as_ref() != Some(&error) {
                        noticed(Noticed::Unrepaired(&error));
                        last = Some(error);
                    }
                }
            }
            drop(alive);
            sleep(self.every).await;
        }
    }
}

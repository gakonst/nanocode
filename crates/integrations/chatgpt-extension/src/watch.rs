use crate::{Bridge, Choice, ExtensionLink, cached_default_browser};
use std::{collections::HashSet, path::PathBuf, sync::Weak, time::Duration};
use tokio::{task::spawn_blocking, time::sleep};

#[derive(Clone, Copy, Debug)]
pub enum Noticed<'a> {
    Proxied(&'static str),
    Released(&'static str),
    Failed(&'static str, &'a str),
    Waiting(&'a str),
    Repaired(usize),
    Unrepaired(&'a str),
    Broken(&'a str),
}

#[derive(Clone, Debug)]
pub struct Watch {
    bridge: Bridge,
    home: PathBuf,
    preferred: Vec<String>,
    every: Duration,
    choosing: u64,
}

impl Watch {
    pub fn new(bridge: Bridge, home: impl Into<PathBuf>, preferred: &[&str]) -> Self {
        Self {
            bridge,
            home: home.into(),
            preferred: preferred.iter().map(|id| (*id).to_owned()).collect(),
            every: Duration::from_secs(3),
            choosing: 10,
        }
    }

    #[must_use]
    pub const fn every(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }

    #[must_use]
    pub const fn choosing(mut self, rounds: u64) -> Self {
        self.choosing = if rounds == 0 { 1 } else { rounds };
        self
    }

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

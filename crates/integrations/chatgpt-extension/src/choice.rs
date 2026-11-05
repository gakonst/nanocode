use crate::{Bridge, Error, Registration, chatgpt_installed, extension_enabled};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub enum Choice {
    Proxied(&'static str),
    Released(&'static str),
    Failed(&'static str, String),
}

fn ago(time: SystemTime, now: SystemTime) -> String {
    let seconds = now
        .duration_since(time)
        .map_or(0, |elapsed| elapsed.as_secs());
    match seconds {
        0..120 => format!("{seconds} s ago"),
        120..7200 => format!("{} min ago", seconds / 60),
        _ => format!("{} h ago", seconds / 3600),
    }
}

impl Bridge {
    pub fn declined_path(&self, root: &Path) -> Result<PathBuf, Error> {
        Ok(self.record_path(root)?.with_extension("off"))
    }

    pub fn roots(&self) -> Vec<(&'static str, PathBuf)> {
        let mut seen = HashSet::new();
        self.browsers()
            .into_iter()
            .filter_map(|(name, dir)| fs::canonicalize(dir).ok().map(|root| (name, root)))
            .filter(|(_, root)| root.is_dir() && seen.insert(root.clone()))
            .collect()
    }

    pub fn proxy(&self, browser: &str) -> Result<PathBuf, Error> {
        let root = self.browser_root(browser)?;
        let path = self.install(&root, browser, false)?;
        let _ = fs::remove_file(self.declined_path(&root)?);
        Ok(path)
    }

    pub fn decline(&self, browser: &str) -> Result<(), Error> {
        let root = self.browser_root(browser)?;
        let declined = self.declined_path(&root)?;
        if !self.state().is_dir() {
            return Ok(());
        }
        let guard = self.lock()?;
        guard.release(&root)?;
        guard.write_private(&declined, b"{}")?;
        Ok(())
    }

    pub fn not_started(&self) -> Vec<String> {
        let names = self.roots();
        let owner = self.owner_name();
        let now = SystemTime::now();
        let mut result = Vec::new();
        for launch in self.launches() {
            let name = names
                .iter()
                .find(|(_, root)| *root == launch.root)
                .map_or_else(
                    || launch.root.display().to_string(),
                    |(name, _)| (*name).to_owned(),
                );
            match launch.started {
                None => result.push(format!(
                    "{name} has not started {owner}'s ChatGPT connector since it was registered {} ({}). The ChatGPT extension there is off, has not restarted since, or {name} does not load it from that folder: check the extension is on in {name}'s extensions page, quit and reopen {name}, and read the extension's service worker console for a native messaging error",
                    ago(launch.registered, now),
                    launch.manifest.display()
                )),
                Some((launched, error)) if !error.trim().is_empty() => result.push(format!(
                    "{name} started {owner}'s ChatGPT connector {}, and it stopped: {}",
                    ago(launched, now),
                    error.trim()
                )),
                Some(_) => {}
            }
        }
        result
    }

    pub fn choose(&self, preferred: &[&str], default: Option<&str>) -> Vec<Choice> {
        self.choose_with(preferred, default, |root, name| {
            self.install(root, name, true)
        })
    }

    fn choose_with(
        &self,
        preferred: &[&str],
        default: Option<&str>,
        mut install: impl FnMut(&Path, &'static str) -> Result<PathBuf, Error>,
    ) -> Vec<Choice> {
        let mut choices = Vec::new();
        let browsers = self.roots();
        let record = |root: &Path| -> Option<Registration> { self.registration(root).ok() };
        let elsewhere = browsers.iter().any(|(_, root)| {
            preferred
                .iter()
                .any(|id| extension_enabled(root, id).unwrap_or(false))
        });
        let explicit = browsers
            .iter()
            .any(|(_, root)| record(root).is_some_and(|record| !record.automatic));
        let mut chosen = None;
        if !elsewhere && !explicit {
            let mut order: Vec<&(&'static str, PathBuf)> = browsers.iter().collect();
            order.sort_by_key(|(name, _)| Some(*name) != default);
            for (name, root) in order {
                if record(root).is_some_and(|record| record.automatic) {
                    chosen = Some(root.clone());
                    break;
                }
                let candidate = (|| -> Result<bool, Error> {
                    Ok(!self.declined_path(root)?.exists() && chatgpt_installed(root)?)
                })();
                match candidate {
                    Ok(true) => match install(root, name) {
                        Ok(_) => {
                            choices.push(Choice::Proxied(name));
                            chosen = Some(root.clone());
                            break;
                        }
                        Err(error) => choices.push(Choice::Failed(name, error.to_string())),
                    },
                    Ok(false) => {}
                    Err(error) => choices.push(Choice::Failed(name, error.to_string())),
                }
            }
        }
        for (name, root) in &browsers {
            if record(root).is_some_and(|record| record.automatic)
                && chosen.as_deref() != Some(root.as_path())
            {
                match self.lock().and_then(|guard| guard.release(root)) {
                    Ok(_) => choices.push(Choice::Released(name)),
                    Err(error) => choices.push(Choice::Failed(name, error.to_string())),
                }
            }
        }
        choices
    }
}

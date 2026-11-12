use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Clone, Debug)]
pub struct UserService {
    pub name: String,
    pub home: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ServiceProgram {
    pub description: String,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub log: PathBuf,
    pub restart_seconds: u32,
    pub interactive: bool,
    pub restart_note: Option<String>,
    pub interactive_note: Option<String>,
}

impl UserService {
    pub fn definition_path(&self) -> PathBuf {
        #[cfg(target_os = "macos")]
        return self
            .home
            .join(format!("Library/LaunchAgents/{}.plist", self.name));
        #[cfg(not(target_os = "macos"))]
        {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| self.home.join(".config"));
            config.join("systemd/user").join(&self.name)
        }
    }

    pub fn installed(&self) -> bool {
        self.definition_path().exists()
    }

    pub fn install(&self, program: &ServiceProgram) -> io::Result<()> {
        let path = self.definition_path();
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("the service definition has no directory"))?,
        )?;
        #[cfg(target_os = "macos")]
        {
            let definition = program.launchd_plist(&self.name)?;
            if let Some(logs) = program.log.parent() {
                std::fs::create_dir_all(logs)?;
            }
            std::fs::write(&path, definition)?;
            let domain = domain();
            quiet(Command::new("launchctl").args(["bootout", &format!("{domain}/{}", self.name)]));
            run(Command::new("launchctl")
                .args(["bootstrap", &domain])
                .arg(&path))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let definition = program.systemd_unit()?;
            let was_active = self.active();
            std::fs::write(&path, definition)?;
            run(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
            run(Command::new("systemctl").args(["--user", "enable", "--now", &self.name]))?;
            if was_active {
                run(Command::new("systemctl").args(["--user", "restart", &self.name]))?;
            }
            Ok(())
        }
    }

    pub fn restart(&self) -> io::Result<bool> {
        if !self.installed() {
            return Ok(false);
        }
        #[cfg(target_os = "macos")]
        {
            let target = format!("{}/{}", domain(), self.name);
            if !quiet(Command::new("launchctl").args(["print", &target])) {
                return Ok(false);
            }
            run(Command::new("launchctl").args(["kickstart", "-k", &target]))?;
            Ok(true)
        }
        #[cfg(not(target_os = "macos"))]
        {
            if !self.active() {
                return Ok(false);
            }
            run(Command::new("systemctl").args(["--user", "restart", &self.name]))?;
            Ok(true)
        }
    }

    pub fn uninstall(&self) -> io::Result<bool> {
        let path = self.definition_path();
        let exists = path.exists();
        #[cfg(target_os = "macos")]
        quiet(Command::new("launchctl").args(["bootout", &format!("{}/{}", domain(), self.name)]));
        #[cfg(not(target_os = "macos"))]
        if exists {
            quiet(Command::new("systemctl").args(["--user", "disable", "--now", &self.name]));
        }
        if exists {
            std::fs::remove_file(&path)?;
            #[cfg(not(target_os = "macos"))]
            quiet(Command::new("systemctl").args(["--user", "daemon-reload"]));
        }
        Ok(exists)
    }

    pub fn state(&self) -> String {
        #[cfg(target_os = "macos")]
        return if quiet(
            Command::new("launchctl").args(["print", &format!("{}/{}", domain(), self.name)]),
        ) {
            "loaded"
        } else {
            "not loaded"
        }
        .to_owned();
        #[cfg(not(target_os = "macos"))]
        Command::new("systemctl")
            .args(["--user", "is-active", &self.name])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into())
    }

    #[cfg(not(target_os = "macos"))]
    fn active(&self) -> bool {
        quiet(Command::new("systemctl").args(["--user", "is-active", "--quiet", &self.name]))
    }
}

impl ServiceProgram {
    pub fn launchd_plist(&self, label: &str) -> io::Result<String> {
        fn escape(value: &str) -> String {
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        }
        let label = escape(text(OsStr::new(label))?);
        let arguments = std::iter::once(self.program.as_os_str())
            .chain(self.args.iter().map(OsString::as_os_str))
            .map(|argument| {
                Ok(format!(
                    "    <string>{}</string>\n",
                    escape(text(argument)?)
                ))
            })
            .collect::<io::Result<String>>()?;
        let variables = self
            .environment
            .iter()
            .map(|(key, value)| {
                Ok(format!(
                    "    <key>{}</key>\n    <string>{}</string>\n",
                    escape(text(key)?),
                    escape(text(value)?)
                ))
            })
            .collect::<io::Result<String>>()?;
        let environment = if self.environment.is_empty() {
            String::new()
        } else {
            format!("  <key>EnvironmentVariables</key>\n  <dict>\n{variables}  </dict>\n")
        };
        let log = escape(text(self.log.as_os_str())?);
        let note = |note: &Option<String>| -> io::Result<String> {
            match note {
                Some(note) => Ok(format!("  <!-- {} -->\n", comment(note)?)),
                None => Ok(String::new()),
            }
        };
        let restart_note = note(&self.restart_note)?;
        let process = if self.interactive {
            format!(
                "{}  <key>ProcessType</key>\n  <string>Interactive</string>\n",
                note(&self.interactive_note)?
            )
        } else {
            String::new()
        };
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{arguments}  </array>
{environment}  <key>RunAtLoad</key>
  <true/>
{restart_note}  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>{seconds}</integer>
{process}  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
            seconds = self.restart_seconds,
        ))
    }

    pub fn systemd_unit(&self) -> io::Result<String> {
        fn quote(value: &str) -> String {
            let escaped = value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
                .replace('$', "$$");
            format!("\"{escaped}\"")
        }
        fn word(value: &str) -> String {
            if !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=,+@".contains(c))
            {
                value.to_owned()
            } else {
                quote(value)
            }
        }
        let mut command = quote(text(self.program.as_os_str())?);
        for argument in &self.args {
            command.push(' ');
            command.push_str(&word(text(argument)?));
        }
        let environment = self
            .environment
            .iter()
            .map(|(key, value)| {
                Ok(format!(
                    "Environment={}\n",
                    quote(&format!("{}={}", text(key)?, text(value)?))
                ))
            })
            .collect::<io::Result<String>>()?;
        let description = text(OsStr::new(&self.description))?;
        let restart_note = match &self.restart_note {
            Some(note) => format!("# {}\n", comment(note)?),
            None => String::new(),
        };
        Ok(format!(
            "[Unit]\n\
             Description={description}\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={command}\n\
             {environment}\
             {restart_note}\
             Restart=on-failure\n\
             RestartSec={seconds}\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            seconds = self.restart_seconds,
        ))
    }
}

fn text(value: &OsStr) -> io::Result<&str> {
    value
        .to_str()
        .filter(|s| !s.chars().any(char::is_control))
        .ok_or_else(|| {
            io::Error::other(format!(
                "{} must be UTF-8 without control characters",
                Path::new(value).display()
            ))
        })
}

fn comment(note: &str) -> io::Result<&str> {
    if note.contains("--") {
        return Err(io::Error::other(format!(
            "{note:?} cannot be a comment in a service definition"
        )));
    }
    text(OsStr::new(note))
}

#[cfg(target_os = "macos")]
fn domain() -> String {
    format!("gui/{}", nanocode_platform::unix::user_id())
}

fn run(command: &mut Command) -> io::Result<()> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| io::Error::other(format!("{command:?}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "{command:?} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn quiet(command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

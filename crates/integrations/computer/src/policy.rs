use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

pub const SENSITIVE: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "com.mitchellh.ghostty",
    "dev.warp.Warp-Stable",
    "net.kovidgoyal.kitty",
    "io.alacritty",
    "com.github.wez.wezterm",
    "com.apple.keychainaccess",
    "com.apple.Passwords",
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.bitwarden.desktop",
    "com.lastpass.LastPass",
    "com.dashlane.dashlanephonefinal",
    "com.apple.SecurityAgent",
    "com.apple.LocalAuthentication.UIAgent",
    "com.apple.coreservices.uiagent",
    "gnome-terminal-server",
    "kgx",
    "ptyxis",
    "konsole",
    "xterm",
    "uxterm",
    "kitty",
    "alacritty",
    "wezterm-gui",
    "foot",
    "tilix",
    "terminator",
    "xfce4-terminal",
    "lxterminal",
    "mate-terminal",
    "qterminal",
    "keepassxc",
    "seahorse",
    "kwalletmanager5",
    "1password",
    "bitwarden",
    "polkit-gnome-authentication-agent-1",
    "polkit-kde-authentication-agent-1",
    "lxpolkit",
    "gcr-prompter",
    "pinentry-gnome3",
    "pinentry-qt",
    "pinentry-gtk-2",
    "ksshaskpass",
    "ssh-askpass",
    "gnome-terminal",
    "org.gnome.Terminal",
    "org.gnome.Console",
    "org.gnome.Ptyxis",
    "org.kde.konsole",
    "debian-xterm",
    "debian-uxterm",
    "x-terminal-emulator",
    "org.wezfurlong.wezterm",
    "wezterm",
    "footclient",
    "com.gexperts.Tilix",
    "ghostty",
    "urxvt",
    "rxvt-unicode",
    "sakura",
    "guake",
    "tilda",
    "org.kde.yakuake",
    "yakuake",
    "terminology",
    "io.elementary.terminal",
    "com.raggesilver.BlackBox",
    "bash",
    "sh",
    "dash",
    "zsh",
    "fish",
    "ksh",
    "tcsh",
    "csh",
    "org.keepassxc.KeePassXC",
    "org.gnome.seahorse.Application",
    "org.gnome.World.Secrets",
    "org.kde.kwalletmanager5",
    "org.kde.kwalletmanager",
    "kwalletmanager",
    "enpass",
    "gnome-control-center",
    "org.gnome.Settings",
    "systemsettings",
    "org.kde.systemsettings",
    "xfce4-settings-manager",
    "lxqt-config",
    "mate-control-center",
    "cinnamon-settings",
    "polkit-mate-authentication-agent-1",
    "org.kde.polkit-kde-authentication-agent-1",
    "lxqt-policykit-agent",
    "xfce-polkit",
    "gcr4-ssh-askpass",
    "pinentry",
    "pinentry-gtk",
    "pinentry-qt5",
    "ssh-askpass-gnome",
    "gnome-ssh-askpass",
    "x11-ssh-askpass",
    "lxqt-openssh-askpass",
    "gnome-software",
    "org.gnome.Software",
    "plasma-discover",
    "org.kde.discover",
    "synaptic",
    "update-manager",
    "software-properties-gtk",
    "gparted",
    "pkexec",
    "sudo",
    "su",
    "WindowsTerminal.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "1Password.exe",
    "Bitwarden.exe",
    "KeePass.exe",
    "KeePassXC.exe",
    "consent.exe",
    "CredentialUIBroker.exe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppIdentity<'a> {
    pub name: &'a str,
    pub id: &'a str,
}

pub type Policy = Arc<dyn Fn(AppIdentity<'_>) -> Result<(), String> + Send + Sync>;

pub fn sensitive(id: &str) -> bool {
    SENSITIVE.iter().any(|known| known.eq_ignore_ascii_case(id))
}

pub struct Leases {
    idle: Duration,
    held: HashMap<String, Lease>,
}

struct Lease {
    session: String,
    turn: String,
    used: Instant,
}

impl Leases {
    pub fn new(idle: Duration) -> Self {
        Self {
            idle,
            held: HashMap::new(),
        }
    }

    pub fn take(&mut self, app: &str, session: &str, turn: &str, now: Instant) -> bool {
        let idle = self.idle;
        self.held
            .retain(|_, lease| now.duration_since(lease.used) < idle);
        let key = app_key(app);
        if self
            .held
            .get(&key)
            .is_some_and(|lease| lease.session != session)
        {
            return false;
        }
        self.held.insert(
            key,
            Lease {
                session: session.to_owned(),
                turn: turn.to_owned(),
                used: now,
            },
        );
        true
    }

    pub fn end(&mut self, session: &str, turn: &str) {
        self.held
            .retain(|_, lease| lease.session != session || lease.turn != turn);
    }
}

pub fn app_key(app: &str) -> String {
    let name = app.trim().trim_end_matches('/');
    let name = name.rsplit('/').next().unwrap_or(name).to_lowercase();
    let name = name.strip_suffix(".app").unwrap_or(&name);
    match name {
        "chrome" | "com.google.chrome" => "google chrome",
        "chrome beta" | "com.google.chrome.beta" => "google chrome beta",
        "chrome canary" | "com.google.chrome.canary" => "google chrome canary",
        "chromium" | "org.chromium.chromium" => "chromium",
        "arc" | "company.thebrowser.browser" => "arc",
        "brave" | "com.brave.browser" => "brave browser",
        "edge" | "com.microsoft.edgemac" => "microsoft edge",
        "safari" | "com.apple.safari" => "safari",
        "firefox" | "org.mozilla.firefox" => "firefox",
        "dia" | "company.thebrowser.dia" => "dia",
        other => other,
    }
    .to_owned()
}

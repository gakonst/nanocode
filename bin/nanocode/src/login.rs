use nanocode::{
    claude::{ClaudeHomeLogin, subscription::ClaudeSubscriptionStatus},
    codex::auth::{ChatGptLogin, chatgpt_auth_file},
};
use nanocode_tui::transcript::NoticeLevel;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{App, AppEvent};

impl App {
    pub(crate) fn login(&mut self, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "login", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let tx = self.tx.clone();
        match argument.trim() {
            "codex" | "chatgpt" | "openai" => {
                tokio::spawn(codex(tx));
            }
            "claude" | "anthropic" => {
                tokio::spawn(claude(tx));
            }
            "tempo" => {
                tokio::spawn(tempo(tx));
            }
            _ => self.notice(NoticeLevel::Info, "usage: /login codex|claude|tempo"),
        }
    }
}

async fn codex(tx: UnboundedSender<AppEvent>) {
    let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
    let path = match chatgpt_auth_file() {
        Ok(path) => path,
        Err(error) => return notice(NoticeLevel::Error, format!("login codex: {error}")),
    };
    let login = match ChatGptLogin::start(path).await {
        Ok(login) => login,
        Err(error) => return notice(NoticeLevel::Error, format!("login codex: {error}")),
    };
    notice(
        NoticeLevel::Info,
        browse("ChatGPT", login.authorization_url()),
    );
    match login.complete().await {
        Ok(status) => notice(
            NoticeLevel::Info,
            format!(
                "signed in to ChatGPT as {}",
                status.email.unwrap_or(status.account_id)
            ),
        ),
        Err(error) => notice(NoticeLevel::Error, format!("login codex: {error}")),
    }
}

async fn claude(tx: UnboundedSender<AppEvent>) {
    let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
    let login = match ClaudeHomeLogin::start().await {
        Ok(login) => login,
        Err(error) => return notice(NoticeLevel::Error, format!("login claude: {error}")),
    };
    notice(
        NoticeLevel::Info,
        browse("Claude", login.authorization_url()),
    );
    match login.complete().await {
        Ok(ClaudeSubscriptionStatus::Authenticated { .. }) => {
            notice(NoticeLevel::Info, "signed in to Claude".to_owned());
        }
        Ok(status) => notice(
            NoticeLevel::Error,
            format!("login claude ended as {status:?}"),
        ),
        Err(error) => notice(NoticeLevel::Error, format!("login claude: {error}")),
    }
}

async fn tempo(tx: UnboundedSender<AppEvent>) {
    let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
    let started = match tempo_wallet(&["wallet", "login", "--no-wait", "--no-browser"]).await {
        Ok(started) => started,
        Err(error) => return notice(NoticeLevel::Error, format!("login tempo: {error}")),
    };
    if let Some(wallet) = started["wallet"].as_str() {
        return notice(
            NoticeLevel::Info,
            format!("signed in to the Tempo wallet {wallet}; --gateway tempo pays with it"),
        );
    }
    let (Some(url), Some(code)) = (
        started["verification_url"].as_str(),
        started["user_code"].as_str(),
    ) else {
        return notice(
            NoticeLevel::Error,
            format!("login tempo: tempo wallet login answered {started}"),
        );
    };
    notice(
        NoticeLevel::Info,
        format!("{}; check that it shows {code}", browse("Tempo", url)),
    );
    match tempo_wallet(&["wallet", "login", "status"]).await {
        Ok(status) => notice(
            NoticeLevel::Info,
            format!(
                "signed in to the Tempo wallet {}; --gateway tempo pays with it",
                status["accounts"][0].as_str().unwrap_or_default()
            ),
        ),
        Err(error) => notice(NoticeLevel::Error, format!("login tempo: {error}")),
    }
}

async fn tempo_wallet(args: &[&str]) -> Result<serde_json::Value, String> {
    let candidates = std::env::var_os("HOME")
        .map(|home| std::path::Path::new(&home).join(".tempo/bin/tempo"))
        .into_iter()
        .chain([std::path::PathBuf::from("tempo")]);
    let mut ran = None;
    for program in candidates {
        match tokio::process::Command::new(&program)
            .args(args)
            .args(["--format", "json"])
            .output()
            .await
        {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            output => {
                ran = Some((program, output));
                break;
            }
        }
    }
    let Some((program, output)) = ran else {
        return Err(
            "the Tempo CLI is not installed: curl -fsSL https://tempo.xyz/install | bash"
                .to_owned(),
        );
    };
    let output = output.map_err(|error| format!("running {}: {error}", program.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "{} {} exited with {}: {}",
            program.display(),
            args.join(" "),
            output.status,
            [stderr.trim(), stdout.trim()].join(" ").trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "{} {} printed no JSON: {error}",
            program.display(),
            args.join(" ")
        )
    })
}

fn browse(service: &str, url: &str) -> String {
    if crate::extras::open_url(url) {
        format!("opened {service} sign-in in your browser; if it did not appear, visit {url}")
    } else {
        format!("visit {url} to sign in to {service}")
    }
}

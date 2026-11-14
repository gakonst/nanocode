#![cfg(not(target_family = "wasm"))]

use nanocode_sync::Atom;
use tokio::sync::mpsc::UnboundedSender;

pub mod accounts;
pub mod charge;
pub mod error;
pub mod events;
pub mod flow;
pub mod identity;
mod keys;
pub mod mcp;
pub mod payer;
pub mod protocol;
pub mod provider;
pub mod proxy;
mod rpc;
pub mod session;
mod signing;
pub mod store;
pub mod tx;
mod x509;

pub type Listeners = Atom<Vec<UnboundedSender<String>>>;

pub fn notify(listeners: &Listeners, notice: Option<String>) {
    let Some(notice) = notice else {
        return;
    };
    let gone = listeners.read(|listeners| {
        listeners
            .iter()
            .filter(|listener| listener.send(notice.clone()).is_err())
            .count()
    });
    if gone > 0 {
        listeners.update(|listeners| listeners.retain(|listener| !listener.is_closed()));
    }
}

pub fn http_client(
    slot: &Atom<Option<nanocode_http::Client>>,
) -> Result<nanocode_http::Client, String> {
    if let Some(client) = slot.read(Clone::clone) {
        return Ok(client);
    }
    let client = nanocode_http::Client::builder()
        .redirect(nanocode_http::redirect::Policy::none())
        .default_headers(nanocode_http::accept_encoding(
            nanocode_http::header::HeaderMap::new(),
        ))
        .build()
        .map_err(|error| error.to_string())?;
    Ok(slot.update(|slot| slot.get_or_insert_with(|| client.clone()).clone()))
}

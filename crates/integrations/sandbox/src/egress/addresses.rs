use nanocode_sync::{Atom, Signal};
use std::{collections::HashMap, net::IpAddr};

#[derive(Clone)]
pub struct Holder {
    pub name: String,
    pub addresses: Vec<IpAddr>,
}

type Held = HashMap<String, Holder>;

#[derive(Default)]
pub struct Addresses {
    held: Atom<Option<Held>>,
    synced: Signal,
}

impl Addresses {
    #[must_use]
    pub fn name(&self, address: IpAddr) -> Option<String> {
        let address = address.to_canonical();
        self.held.read(|held| {
            let mut owners = held
                .as_ref()?
                .values()
                .filter(|holder| holder.addresses.contains(&address))
                .map(|holder| &holder.name);
            let owner = owners.next()?;
            owners.next().is_none().then(|| owner.clone())
        })
    }

    pub fn listed(&self, held: impl IntoIterator<Item = (String, Holder)>, most: usize) {
        let held: Held = held
            .into_iter()
            .map(|(key, holder)| (key, canonical(holder)))
            .take(most)
            .collect();
        self.held.set(Some(held));
        self.synced.fire();
    }

    pub fn changed(&self, key: &str, holder: Option<Holder>, most: usize) {
        let holder = holder.map(canonical);
        self.held.update(|held| {
            let Some(held) = held else { return };
            match holder.clone() {
                Some(holder) if held.len() < most || held.contains_key(key) => {
                    drop(held.insert(key.to_owned(), holder));
                }
                _ => drop(held.remove(key)),
            }
        });
    }

    pub fn lost(&self) {
        self.held.set(None);
    }

    pub async fn synced(&self) {
        self.synced.wait().await;
    }
}

fn canonical(holder: Holder) -> Holder {
    Holder {
        addresses: holder
            .addresses
            .into_iter()
            .map(|ip| ip.to_canonical())
            .collect(),
        ..holder
    }
}

use crate::egress::wire::{Credential, Grant, Report};
use std::{error::Error, future::Future};

pub type Failure = Box<dyn Error + Send + Sync>;

pub trait Authority: Send + Sync + 'static {
    fn grant(
        &self,
        credential: &Credential,
    ) -> impl Future<Output = Result<Option<Grant>, Failure>> + Send;

    fn report(&self, report: &Report) -> impl Future<Output = Result<(), Failure>> + Send;

    fn cached(&self) -> bool {
        true
    }
}

pub struct Fixed(pub Grant);

impl Authority for Fixed {
    async fn grant(&self, _: &Credential) -> Result<Option<Grant>, Failure> {
        Ok(Some(self.0.clone()))
    }

    async fn report(&self, _: &Report) -> Result<(), Failure> {
        Ok(())
    }

    fn cached(&self) -> bool {
        false
    }
}

#[must_use]
pub fn chain(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        text.push_str(": ");
        text.push_str(&next.to_string());
        cause = next.source();
    }
    text
}

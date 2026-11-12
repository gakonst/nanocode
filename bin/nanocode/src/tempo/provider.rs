use std::future::Future;
use std::pin::Pin;

use reqwest::Url;
use reqwest::header::HeaderMap;

use super::error::Fault;
use super::protocol::{Challenge, Credential, Receipt};

pub(crate) type PaymentEventFuture = Pin<Box<dyn Future<Output = Result<(), Fault>> + Send>>;
pub(crate) type ManagementFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<Receipt>, Fault>> + Send + 'a>>;

pub(crate) trait ManagementTransport: Send + Sync {
    fn send<'a>(
        &'a self,
        challenge: &'a Challenge,
        credential: &'a Credential,
    ) -> ManagementFuture<'a>;
}

#[derive(Clone, Debug)]
pub(crate) struct PaymentContext {
    pub(crate) url: Url,
    pub(crate) headers: HeaderMap,
    pub(crate) http_client: Option<reqwest::Client>,
}

pub(crate) trait PaymentProvider: Clone + Send + Sync {
    fn supports(&self, method: &str, intent: &str) -> bool;

    fn select_challenge<'a>(&self, challenges: &[&'a Challenge]) -> Option<&'a Challenge>;

    fn pay_with_context(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
    ) -> impl Future<Output = Result<Credential, Fault>> + Send;

    fn pay_with_management(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
        management: &dyn ManagementTransport,
    ) -> impl Future<Output = Result<Credential, Fault>> + Send;

    fn commit_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> impl Future<Output = Result<(), Fault>> + Send;

    fn rollback_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> impl Future<Output = Result<(), Fault>> + Send;

    fn invalidate_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> impl Future<Output = Result<(), Fault>> + Send;

    fn abandon_payment(&self, challenge: &Challenge, credential: &Credential);

    fn prepaid_credential(
        &self,
        context: PaymentContext,
    ) -> impl Future<Output = Option<(Challenge, Credential)>> + Send;

    fn record_receipt(
        &self,
        context: &PaymentContext,
        challenge: &Challenge,
        receipt: &Receipt,
    ) -> Option<u128>;

    fn funding_currency(&self, challenge: &Challenge) -> Option<String>;

    fn stream_event(
        &self,
        client: &reqwest::Client,
        url: &str,
        name: &str,
        data: &str,
    ) -> Option<PaymentEventFuture>;

    fn accept_payment_header(&self) -> Option<String>;
}

pub(crate) struct PendingPayment<P: PaymentProvider> {
    provider: P,
    challenge: Challenge,
    credential: Credential,
    active: bool,
}

impl<P: PaymentProvider> PendingPayment<P> {
    pub(crate) const fn new(provider: P, challenge: Challenge, credential: Credential) -> Self {
        Self {
            provider,
            challenge,
            credential,
            active: true,
        }
    }

    pub(crate) const fn challenge(&self) -> &Challenge {
        &self.challenge
    }

    pub(crate) const fn credential(&self) -> &Credential {
        &self.credential
    }

    pub(crate) async fn commit(mut self) -> Result<(), Fault> {
        self.provider
            .commit_payment(&self.challenge, &self.credential)
            .await?;
        self.active = false;
        Ok(())
    }

    pub(crate) async fn rollback(mut self) -> Result<(), Fault> {
        self.provider
            .rollback_payment(&self.challenge, &self.credential)
            .await?;
        self.active = false;
        Ok(())
    }

    pub(crate) async fn invalidate(mut self) -> Result<(), Fault> {
        self.provider
            .invalidate_payment(&self.challenge, &self.credential)
            .await?;
        self.active = false;
        Ok(())
    }
}

impl<P: PaymentProvider> Drop for PendingPayment<P> {
    fn drop(&mut self) {
        if self.active {
            self.provider
                .abandon_payment(&self.challenge, &self.credential);
        }
    }
}

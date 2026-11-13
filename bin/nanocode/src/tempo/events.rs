use std::sync::Arc;

use nanocode_http::Url;
use nanocode_sync::Atom;

use super::protocol::{Challenge, Receipt};

#[derive(Debug, Clone)]
pub(crate) struct PaymentResponse {
    pub(crate) challenge: Challenge,
    pub(crate) url: Url,
    pub(crate) receipt: Option<Receipt>,
    pub(crate) amount: Option<u128>,
    pub(crate) funding_currency: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Deposit {
    pub(crate) url: Url,
    pub(crate) channel_id: String,
    pub(crate) currency: Option<String>,
    pub(crate) amount: u128,
    pub(crate) deposit: u128,
    pub(crate) opened: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PaymentUpdate {
    pub(crate) url: Url,
    pub(crate) change: i128,
}

type Handlers<T> = Atom<Vec<Arc<dyn Fn(&T) + Send + Sync>>>;

#[derive(Default)]
struct Listeners {
    responses: Handlers<PaymentResponse>,
    deposits: Handlers<Deposit>,
    updates: Handlers<PaymentUpdate>,
}

#[derive(Clone, Default)]
pub(crate) struct Events(Arc<Listeners>);

fn emit<T>(handlers: &Handlers<T>, event: &T) {
    for handler in handlers.get().iter() {
        handler(event);
    }
}

impl Events {
    pub(crate) fn on_payment_response(
        &self,
        handler: impl Fn(&PaymentResponse) + Send + Sync + 'static,
    ) {
        let handler: Arc<dyn Fn(&PaymentResponse) + Send + Sync> = Arc::new(handler);
        self.0
            .responses
            .update(|handlers| handlers.push(handler.clone()));
    }

    pub(crate) fn on_deposit(&self, handler: impl Fn(&Deposit) + Send + Sync + 'static) {
        let handler: Arc<dyn Fn(&Deposit) + Send + Sync> = Arc::new(handler);
        self.0
            .deposits
            .update(|handlers| handlers.push(handler.clone()));
    }

    pub(crate) fn on_payment_update(
        &self,
        handler: impl Fn(&PaymentUpdate) + Send + Sync + 'static,
    ) {
        let handler: Arc<dyn Fn(&PaymentUpdate) + Send + Sync> = Arc::new(handler);
        self.0
            .updates
            .update(|handlers| handlers.push(handler.clone()));
    }

    pub(crate) fn payment_response(&self, event: &PaymentResponse) {
        emit(&self.0.responses, event);
    }

    pub(crate) fn deposit(&self, event: &Deposit) {
        emit(&self.0.deposits, event);
    }

    pub(crate) fn payment_update(&self, event: &PaymentUpdate) {
        emit(&self.0.updates, event);
    }
}

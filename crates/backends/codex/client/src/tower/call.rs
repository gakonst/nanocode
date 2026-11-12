use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, ready},
};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub trait Service<Request> {
    type Response;
    type Error;
    type Future: Future<Output = Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>>;

    fn call(&mut self, request: Request) -> Self::Future;
}

pub trait Layer<S> {
    type Service;

    fn layer(&self, inner: S) -> Self::Service;
}

pub async fn ready<S, Request>(service: &mut S) -> Result<&mut S, S::Error>
where
    S: Service<Request>,
{
    std::future::poll_fn(|context| service.poll_ready(context)).await?;
    Ok(service)
}

pub trait Policy<Request, Response, Error> {
    type Future: Future<Output = ()>;

    fn retry(
        &mut self,
        request: &mut Request,
        result: &mut Result<Response, Error>,
    ) -> Option<Self::Future>;

    fn clone_request(&mut self, request: &Request) -> Option<Request>;
}

#[derive(Clone, Debug)]
pub struct Retry<P, S> {
    policy: P,
    service: S,
}

impl<P, S> Retry<P, S> {
    pub const fn new(policy: P, service: S) -> Self {
        Self { policy, service }
    }

    pub const fn get_ref(&self) -> &S {
        &self.service
    }

    pub const fn get_mut(&mut self) -> &mut S {
        &mut self.service
    }

    pub fn into_inner(self) -> S {
        self.service
    }
}

impl<P, S, Request> Service<Request> for Retry<P, S>
where
    P: Policy<Request, S::Response, S::Error> + Clone + Unpin,
    S: Service<Request> + Clone + Unpin,
    S::Future: Unpin,
    P::Future: Unpin,
    Request: Unpin,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Retrying<P, S, Request>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(context)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let cloned = self.policy.clone_request(&request);
        let future = self.service.call(request);
        Retrying {
            request: cloned,
            retry: self.clone(),
            state: RetryState::Called(future),
        }
    }
}

enum RetryState<F, W> {
    Called(F),
    Waiting(W),
    Retrying,
}

pub struct Retrying<P, S, Request>
where
    P: Policy<Request, S::Response, S::Error>,
    S: Service<Request>,
{
    request: Option<Request>,
    retry: Retry<P, S>,
    state: RetryState<S::Future, P::Future>,
}

impl<P, S, Request> Future for Retrying<P, S, Request>
where
    P: Policy<Request, S::Response, S::Error> + Unpin,
    S: Service<Request> + Unpin,
    S::Future: Unpin,
    P::Future: Unpin,
    Request: Unpin,
{
    type Output = Result<S::Response, S::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                RetryState::Called(future) => {
                    let mut result = ready!(Pin::new(future).poll(context));
                    let Some(request) = &mut this.request else {
                        return Poll::Ready(result);
                    };
                    match this.retry.policy.retry(request, &mut result) {
                        Some(waiting) => this.state = RetryState::Waiting(waiting),
                        None => return Poll::Ready(result),
                    }
                }
                RetryState::Waiting(waiting) => {
                    ready!(Pin::new(waiting).poll(context));
                    this.state = RetryState::Retrying;
                }
                RetryState::Retrying => {
                    ready!(this.retry.service.poll_ready(context))?;
                    let request = this
                        .request
                        .take()
                        .expect("a retry keeps the cloned request");
                    this.request = this.retry.policy.clone_request(&request);
                    this.state = RetryState::Called(this.retry.service.call(request));
                }
            }
        }
    }
}

use std::sync::Arc;

use nanocode_http::layer::{BoxFuture, Error, Layer, Next, Request, Response};
use nanocode_http::types::{
    HeaderValue,
    header::{AUTHORIZATION, InvalidHeaderValue},
};
use serde::Deserialize;

const MODEL_FIELD: &[u8] = b"\"model\":\"";
const NAMED_WITHIN: usize = 256;

#[derive(Clone)]
pub struct ModelRoute {
    url: String,
    authorization: HeaderValue,
    models: Arc<[String]>,
}

#[derive(Deserialize)]
struct Body {
    model: Option<String>,
}

fn named(body: &[u8]) -> Option<&str> {
    let head = body.get(..body.len().min(NAMED_WITHIN))?;
    let start = head
        .windows(MODEL_FIELD.len())
        .position(|window| window == MODEL_FIELD)?
        + MODEL_FIELD.len();
    let length = head.get(start..)?.iter().position(|byte| *byte == b'"')?;
    std::str::from_utf8(head.get(start..start + length)?).ok()
}

impl ModelRoute {
    #[must_use]
    pub fn new(
        models: impl Into<Arc<[String]>>,
        url: impl Into<String>,
        authorization: HeaderValue,
    ) -> Self {
        Self {
            url: url.into(),
            authorization,
            models: models.into(),
        }
    }

    pub fn bearer(
        models: impl Into<Arc<[String]>>,
        url: impl Into<String>,
        key: &str,
    ) -> Result<Self, InvalidHeaderValue> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {key}"))?;
        authorization.set_sensitive(true);
        Ok(Self::new(models, url, authorization))
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub const fn authorization(&self) -> &HeaderValue {
        &self.authorization
    }

    #[must_use]
    pub fn models(&self) -> &[String] {
        &self.models
    }

    fn reroute(&self, request: &mut Request) {
        let model = named(&request.body).map(str::to_owned).or_else(|| {
            serde_json::from_slice::<Body>(&request.body)
                .ok()
                .and_then(|body| body.model)
        });
        if !model.is_some_and(|model| self.models.contains(&model)) {
            return;
        }
        let tail = request.url.rsplit_once('/').map_or("", |(_, tail)| tail);
        request.url = format!("{}/{tail}", self.url.trim_end_matches('/'));
        request
            .headers
            .insert(AUTHORIZATION, self.authorization.clone());
    }
}

impl Layer for ModelRoute {
    fn send<'a>(
        &'a self,
        mut request: Request,
        next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, Error>> {
        self.reroute(&mut request);
        next.send(request)
    }
}

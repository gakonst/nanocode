use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    error::Error,
};

use serde_json::Value;
use std::sync::LazyLock;
use url::Url;

use crate::{
    UrlPtr,
    append_list::AppendList,
    compiler::CompileError,
    draft::{Draft, latest},
    util::split,
};

pub trait UrlLoader {
    fn load(&self, url: &str) -> Result<Value, Box<dyn Error>>;
}

#[derive(Default)]
pub struct SchemeUrlLoader {
    loaders: HashMap<&'static str, Box<dyn UrlLoader>>,
}

impl SchemeUrlLoader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, scheme: &'static str, url_loader: Box<dyn UrlLoader>) {
        self.loaders.insert(scheme, url_loader);
    }
}

impl UrlLoader for SchemeUrlLoader {
    fn load(&self, url: &str) -> Result<Value, Box<dyn Error>> {
        let url = Url::parse(url)?;
        let Some(loader) = self.loaders.get(url.scheme()) else {
            return Err(CompileError::UnsupportedUrlScheme {
                url: url.as_str().to_owned(),
            }
            .into());
        };
        loader.load(url.as_str())
    }
}

pub(crate) struct DefaultUrlLoader {
    doc_map: RefCell<HashMap<Url, usize>>,
    doc_list: AppendList<Value>,
    loader: Box<dyn UrlLoader>,
}

impl DefaultUrlLoader {
    pub fn new() -> Self {
        let loader = SchemeUrlLoader::new();
        Self {
            doc_map: Default::default(),
            doc_list: AppendList::new(),
            loader: Box::new(loader),
        }
    }

    pub fn get_doc(&self, url: &Url) -> Option<&Value> {
        self.doc_map
            .borrow()
            .get(url)
            .and_then(|i| self.doc_list.get(*i))
    }

    pub fn add_doc(&self, url: Url, json: Value) {
        if self.get_doc(&url).is_some() {
            return;
        }
        self.doc_list.push(json);
        self.doc_map
            .borrow_mut()
            .insert(url, self.doc_list.len() - 1);
    }

    pub fn use_loader(&mut self, loader: Box<dyn UrlLoader>) {
        self.loader = loader;
    }

    pub(crate) fn load(&self, url: &Url) -> Result<&Value, CompileError> {
        if let Some(doc) = self.get_doc(url) {
            return Ok(doc);
        }

        let doc = if let Some(content) = load_std_meta(url.as_str()) {
            serde_json::from_str::<Value>(content).map_err(|e| CompileError::LoadUrlError {
                url: url.to_string(),
                src: e.into(),
            })?
        } else {
            self.loader
                .load(url.as_str())
                .map_err(|src| CompileError::LoadUrlError {
                    url: url.as_str().to_owned(),
                    src,
                })?
        };
        self.add_doc(url.clone(), doc);
        self.get_doc(url)
            .ok_or(CompileError::Bug("doc must exist".into()))
    }

    pub(crate) fn get_draft(
        &self,
        up: &UrlPtr,
        doc: &Value,
        default_draft: &'static Draft,
        mut cycle: HashSet<Url>,
    ) -> Result<&'static Draft, CompileError> {
        let Value::Object(obj) = &doc else {
            return Ok(default_draft);
        };
        let Some(Value::String(sch)) = obj.get("$schema") else {
            return Ok(default_draft);
        };
        if let Some(draft) = Draft::from_url(sch) {
            return Ok(draft);
        }
        let (sch, _) = split(sch);
        let sch = Url::parse(sch).map_err(|e| CompileError::InvalidMetaSchemaUrl {
            url: up.to_string(),
            src: e.into(),
        })?;
        if up.ptr.is_empty() && sch == up.url {
            return Err(CompileError::UnsupportedDraft { url: sch.into() });
        }
        if !cycle.insert(sch.clone()) {
            return Err(CompileError::MetaSchemaCycle { url: sch.into() });
        }

        let doc = self.load(&sch)?;
        let up = UrlPtr {
            url: sch,
            ptr: "".into(),
        };
        self.get_draft(&up, doc, default_draft, cycle)
    }

    pub(crate) fn get_meta_vocabs(
        &self,
        doc: &Value,
        draft: &'static Draft,
    ) -> Result<Option<Vec<String>>, CompileError> {
        let Value::Object(obj) = &doc else {
            return Ok(None);
        };
        let Some(Value::String(sch)) = obj.get("$schema") else {
            return Ok(None);
        };
        if Draft::from_url(sch).is_some() {
            return Ok(None);
        }
        let (sch, _) = split(sch);
        let sch = Url::parse(sch).map_err(|e| CompileError::ParseUrlError {
            url: sch.to_string(),
            src: e.into(),
        })?;
        let doc = self.load(&sch)?;
        draft.get_vocabs(&sch, doc)
    }
}

pub(crate) static STD_METAFILES: LazyLock<HashMap<String, &str>> = LazyLock::new(|| {
    let mut files = HashMap::new();
    macro_rules! add {
        ($path:expr) => {
            files.insert(
                $path["metaschemas/".len()..].to_owned(),
                include_str!($path),
            );
        };
    }
    add!("metaschemas/draft-04/schema");
    add!("metaschemas/draft-06/schema");
    add!("metaschemas/draft-07/schema");
    add!("metaschemas/draft/2019-09/schema");
    add!("metaschemas/draft/2019-09/meta/core");
    add!("metaschemas/draft/2019-09/meta/applicator");
    add!("metaschemas/draft/2019-09/meta/validation");
    add!("metaschemas/draft/2019-09/meta/meta-data");
    add!("metaschemas/draft/2019-09/meta/format");
    add!("metaschemas/draft/2019-09/meta/content");
    add!("metaschemas/draft/2020-12/schema");
    add!("metaschemas/draft/2020-12/meta/core");
    add!("metaschemas/draft/2020-12/meta/applicator");
    add!("metaschemas/draft/2020-12/meta/unevaluated");
    add!("metaschemas/draft/2020-12/meta/validation");
    add!("metaschemas/draft/2020-12/meta/meta-data");
    add!("metaschemas/draft/2020-12/meta/content");
    add!("metaschemas/draft/2020-12/meta/format-annotation");
    add!("metaschemas/draft/2020-12/meta/format-assertion");
    files
});

fn load_std_meta(url: &str) -> Option<&'static str> {
    let meta = url
        .strip_prefix("http://json-schema.org/")
        .or_else(|| url.strip_prefix("https://json-schema.org/"));
    if let Some(meta) = meta {
        if meta == "schema" {
            return load_std_meta(latest().url);
        }
        return STD_METAFILES.get(meta).cloned();
    }
    None
}

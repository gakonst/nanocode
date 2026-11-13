use std::collections::{HashMap, HashSet};

use crate::{compiler::CompileError, draft::*, util::*};

use nanocode_url::Url;
use serde_json::Value;

pub(crate) struct Root {
    pub(crate) draft: &'static Draft,
    pub(crate) resources: HashMap<JsonPointer, Resource>,
    pub(crate) url: Url,
    pub(crate) meta_vocabs: Option<Vec<String>>,
}

impl Root {
    pub(crate) fn has_vocab(&self, name: &str) -> bool {
        if self.draft.version < 2019 || name == "core" {
            return true;
        }
        if let Some(vocabs) = &self.meta_vocabs {
            return vocabs.iter().any(|s| s == name);
        }
        self.draft.default_vocabs.contains(&name)
    }

    fn resolve_fragment_in(&self, frag: &Fragment, res: &Resource) -> Result<UrlPtr, CompileError> {
        let ptr = match frag {
            Fragment::Anchor(anchor) => {
                let Some(ptr) = res.anchors.get(anchor) else {
                    return Err(CompileError::AnchorNotFound {
                        url: self.url.to_string(),
                        reference: UrlFrag::format(&res.id, frag.as_str()),
                    });
                };
                ptr.clone()
            }
            Fragment::JsonPointer(ptr) => res.ptr.concat(ptr),
        };
        Ok(UrlPtr {
            url: self.url.clone(),
            ptr,
        })
    }

    pub(crate) fn resolve_fragment(&self, frag: &Fragment) -> Result<UrlPtr, CompileError> {
        let res = self.resources.get("").ok_or(CompileError::Bug(
            format!("no root resource found for {}", self.url).into(),
        ))?;
        self.resolve_fragment_in(frag, res)
    }

    pub(crate) fn resolve(&self, uf: &UrlFrag) -> Result<Option<UrlPtr>, CompileError> {
        let res = {
            if uf.url == self.url {
                self.resources.get("").ok_or(CompileError::Bug(
                    format!("no root resource found for {}", self.url).into(),
                ))?
            } else {
                let Some(res) = self.resources.values().find(|res| res.id == uf.url) else {
                    return Ok(None);
                };
                res
            }
        };

        self.resolve_fragment_in(&uf.frag, res).map(Some)
    }

    pub(crate) fn resource(&self, ptr: &JsonPointer) -> &Resource {
        let mut ptr = ptr.as_str();
        loop {
            if let Some(res) = self.resources.get(ptr) {
                return res;
            }
            let Some((prefix, _)) = ptr.rsplit_once('/') else {
                break;
            };
            ptr = prefix;
        }
        self.resources.get("").expect("root resource should exist")
    }

    pub(crate) fn base_url(&self, ptr: &JsonPointer) -> &Url {
        &self.resource(ptr).id
    }

    pub(crate) fn add_subschema(
        &mut self,
        doc: &Value,
        ptr: &JsonPointer,
    ) -> Result<(), CompileError> {
        let v = ptr.lookup(doc, &self.url)?;
        let base_url = self.base_url(ptr).clone();
        self.draft
            .collect_resources(v, &base_url, ptr.clone(), &self.url, &mut self.resources)?;

        if !self.resources.contains_key(ptr) {
            let res = self.resource(ptr);
            if let Some(res) = self.resources.get_mut(&res.ptr.clone()) {
                self.draft.collect_anchors(v, ptr, res, &self.url)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct Resource {
    pub(crate) ptr: JsonPointer,
    pub(crate) id: Url,
    pub(crate) anchors: HashMap<Anchor, JsonPointer>,
    pub(crate) dynamic_anchors: HashSet<Anchor>,
}

impl Resource {
    pub(crate) fn new(ptr: JsonPointer, id: Url) -> Self {
        Self {
            ptr,
            id,
            anchors: HashMap::new(),
            dynamic_anchors: HashSet::new(),
        }
    }
}

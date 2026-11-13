use std::{
    collections::{HashMap, hash_map::Entry},
    str::FromStr,
};

use nanocode_url::Url;
use serde_json::{Map, Value};
use std::sync::LazyLock;

use crate::{SchemaIndex, Schemas, compiler::*, root::Resource, util::*};

const POS_SELF: u8 = 1 << 0;
const POS_PROP: u8 = 1 << 1;
const POS_ITEM: u8 = 1 << 2;

pub(crate) static DRAFT4: LazyLock<Draft> = LazyLock::new(|| Draft {
    version: 4,
    id: "id",
    url: "http://json-schema.org/draft-04/schema",
    subschemas: HashMap::from([
        ("definitions", POS_PROP),
        ("not", POS_SELF),
        ("allOf", POS_ITEM),
        ("anyOf", POS_ITEM),
        ("oneOf", POS_ITEM),
        ("properties", POS_PROP),
        ("additionalProperties", POS_SELF),
        ("patternProperties", POS_PROP),
        ("items", POS_SELF | POS_ITEM),
        ("additionalItems", POS_SELF),
        ("dependencies", POS_PROP),
    ]),
    vocab_prefix: "",
    all_vocabs: vec![],
    default_vocabs: vec![],
});

pub(crate) static DRAFT6: LazyLock<Draft> = LazyLock::new(|| {
    let mut subschemas = DRAFT4.subschemas.clone();
    subschemas.extend([("propertyNames", POS_SELF), ("contains", POS_SELF)]);
    Draft {
        version: 6,
        id: "$id",
        url: "http://json-schema.org/draft-06/schema",
        subschemas,
        vocab_prefix: "",
        all_vocabs: vec![],
        default_vocabs: vec![],
    }
});

pub(crate) static DRAFT7: LazyLock<Draft> = LazyLock::new(|| {
    let mut subschemas = DRAFT6.subschemas.clone();
    subschemas.extend([("if", POS_SELF), ("then", POS_SELF), ("else", POS_SELF)]);
    Draft {
        version: 7,
        id: "$id",
        url: "http://json-schema.org/draft-07/schema",
        subschemas,
        vocab_prefix: "",
        all_vocabs: vec![],
        default_vocabs: vec![],
    }
});

pub(crate) static DRAFT2019: LazyLock<Draft> = LazyLock::new(|| {
    let mut subschemas = DRAFT7.subschemas.clone();
    subschemas.extend([
        ("$defs", POS_PROP),
        ("dependentSchemas", POS_PROP),
        ("unevaluatedProperties", POS_SELF),
        ("unevaluatedItems", POS_SELF),
        ("contentSchema", POS_SELF),
    ]);
    Draft {
        version: 2019,
        id: "$id",
        url: "https://json-schema.org/draft/2019-09/schema",
        subschemas,
        vocab_prefix: "https://json-schema.org/draft/2019-09/vocab/",
        all_vocabs: vec![
            "core",
            "applicator",
            "validation",
            "meta-data",
            "format",
            "content",
        ],
        default_vocabs: vec!["core", "applicator", "validation"],
    }
});

pub(crate) static DRAFT2020: LazyLock<Draft> = LazyLock::new(|| {
    let mut subschemas = DRAFT2019.subschemas.clone();
    subschemas.extend([("prefixItems", POS_ITEM)]);
    Draft {
        version: 2020,
        id: "$id",
        url: "https://json-schema.org/draft/2020-12/schema",
        subschemas,
        vocab_prefix: "https://json-schema.org/draft/2020-12/vocab/",
        all_vocabs: vec![
            "core",
            "applicator",
            "unevaluated",
            "validation",
            "meta-data",
            "format-annotation",
            "format-assertion",
            "content",
        ],
        default_vocabs: vec!["core", "applicator", "unevaluated", "validation"],
    }
});

pub(crate) static STD_METASCHEMAS: LazyLock<Schemas> =
    LazyLock::new(|| load_std_metaschemas().expect("std metaschemas must be compilable"));

pub(crate) fn latest() -> &'static Draft {
    crate::Draft::default().internal()
}

pub(crate) struct Draft {
    pub(crate) version: usize,
    pub(crate) url: &'static str,
    id: &'static str,
    subschemas: HashMap<&'static str, u8>,
    pub(crate) vocab_prefix: &'static str,
    pub(crate) all_vocabs: Vec<&'static str>,
    pub(crate) default_vocabs: Vec<&'static str>,
}

impl Draft {
    pub(crate) fn from_url(url: &str) -> Option<&'static Self> {
        let (mut url, frag) = split(url);
        if !frag.is_empty() {
            return None;
        }
        if let Some(s) = url.strip_prefix("http://") {
            url = s;
        }
        if let Some(s) = url.strip_prefix("https://") {
            url = s;
        }
        match url {
            "json-schema.org/schema" => Some(latest()),
            "json-schema.org/draft/2020-12/schema" => Some(&DRAFT2020),
            "json-schema.org/draft/2019-09/schema" => Some(&DRAFT2019),
            "json-schema.org/draft-07/schema" => Some(&DRAFT7),
            "json-schema.org/draft-06/schema" => Some(&DRAFT6),
            "json-schema.org/draft-04/schema" => Some(&DRAFT4),
            _ => None,
        }
    }

    fn get_schema(&self) -> Option<SchemaIndex> {
        let url = match self.version {
            2020 => "https://json-schema.org/draft/2020-12/schema",
            2019 => "https://json-schema.org/draft/2019-09/schema",
            7 => "http://json-schema.org/draft-07/schema",
            6 => "http://json-schema.org/draft-06/schema",
            4 => "http://json-schema.org/draft-04/schema",
            _ => return None,
        };
        let up = UrlPtr {
            url: Url::parse(url).unwrap_or_else(|_| panic!("{url} should be valid url")),
            ptr: "".into(),
        };
        STD_METASCHEMAS.get_by_loc(&up).map(|s| s.idx)
    }

    pub(crate) fn validate(&self, up: &UrlPtr, v: &Value) -> Result<(), CompileError> {
        let Some(sch) = self.get_schema() else {
            return Err(CompileError::Bug(
                format!("no metaschema preloaded for draft {}", self.version).into(),
            ));
        };
        STD_METASCHEMAS
            .validate(v, sch)
            .map_err(|src| CompileError::ValidationError {
                url: up.to_string(),
                src: src.clone_static(),
            })
    }

    fn get_id<'a>(&self, obj: &'a Map<String, Value>) -> Option<&'a str> {
        if self.version < 2019 && obj.contains_key("$ref") {
            return None;
        }
        let Some(Value::String(id)) = obj.get(self.id) else {
            return None;
        };
        let (id, _) = split(id);
        Some(id).filter(|id| !id.is_empty())
    }

    pub(crate) fn get_vocabs(
        &self,
        url: &Url,
        doc: &Value,
    ) -> Result<Option<Vec<String>>, CompileError> {
        if self.version < 2019 {
            return Ok(None);
        }
        let Value::Object(obj) = doc else {
            return Ok(None);
        };

        let Some(Value::Object(obj)) = obj.get("$vocabulary") else {
            return Ok(None);
        };

        let mut vocabs = vec![];
        for (vocab, reqd) in obj {
            if let Value::Bool(required) = reqd {
                let name = vocab
                    .strip_prefix(self.vocab_prefix)
                    .filter(|name| self.all_vocabs.contains(name));
                if let Some(name) = name {
                    vocabs.push(name.to_owned());
                } else if *required {
                    return Err(CompileError::UnsupportedVocabulary {
                        url: url.as_str().to_owned(),
                        vocabulary: vocab.to_owned(),
                    });
                }
            }
        }
        Ok(Some(vocabs))
    }

    pub(crate) fn collect_anchors(
        &self,
        sch: &Value,
        sch_ptr: &JsonPointer,
        res: &mut Resource,
        url: &Url,
    ) -> Result<(), CompileError> {
        let Value::Object(obj) = sch else {
            return Ok(());
        };

        let mut add_anchor = |anchor: Anchor| match res.anchors.entry(anchor) {
            Entry::Occupied(entry) => {
                if entry.get() == sch_ptr {
                    return Ok(());
                }
                Err(CompileError::DuplicateAnchor {
                    url: url.as_str().to_owned(),
                    anchor: entry.key().to_string(),
                    ptr1: entry.get().to_string(),
                    ptr2: sch_ptr.to_string(),
                })
            }
            entry => {
                entry.or_insert(sch_ptr.to_owned());
                Ok(())
            }
        };

        if self.version < 2019 {
            if obj.contains_key("$ref") {
                return Ok(());
            }

            if let Some(Value::String(id)) = obj.get(self.id) {
                let Ok((_, frag)) = Fragment::split(id) else {
                    let loc = UrlFrag::format(url, sch_ptr.as_str());
                    return Err(CompileError::ParseAnchorError { loc });
                };
                if let Fragment::Anchor(anchor) = frag {
                    add_anchor(anchor)?;
                };
                return Ok(());
            }
        }
        if self.version >= 2019
            && let Some(Value::String(anchor)) = obj.get("$anchor")
        {
            add_anchor(anchor.as_str().into())?;
        }
        if self.version >= 2020
            && let Some(Value::String(anchor)) = obj.get("$dynamicAnchor")
        {
            add_anchor(anchor.as_str().into())?;
            res.dynamic_anchors.insert(anchor.as_str().into());
        }
        Ok(())
    }

    pub(crate) fn collect_resources(
        &self,
        sch: &Value,
        base: &Url,
        sch_ptr: JsonPointer,
        url: &Url,
        resources: &mut HashMap<JsonPointer, Resource>,
    ) -> Result<(), CompileError> {
        if resources.contains_key(&sch_ptr) {
            return Ok(());
        }
        if let Value::Bool(_) = sch {
            if sch_ptr.is_empty() {
                resources.insert(sch_ptr.clone(), Resource::new(sch_ptr, base.clone()));
            }
            return Ok(());
        }

        let Value::Object(obj) = sch else {
            return Ok(());
        };

        let mut base = base;
        let tmp;
        let res = if let Some(id) = self.get_id(obj) {
            let Ok(id) = UrlFrag::join(base, id) else {
                let loc = UrlFrag::format(url, sch_ptr.as_str());
                return Err(CompileError::ParseIdError { loc });
            };
            tmp = id.url;
            base = &tmp;
            Some(Resource::new(sch_ptr.clone(), base.clone()))
        } else if sch_ptr.is_empty() {
            Some(Resource::new(sch_ptr.clone(), base.clone()))
        } else {
            None
        };
        if let Some(res) = res {
            if let Some(dup) = resources.values_mut().find(|res| res.id == *base) {
                return Err(CompileError::DuplicateId {
                    url: url.to_string(),
                    id: base.to_string(),
                    ptr1: res.ptr.to_string(),
                    ptr2: dup.ptr.to_string(),
                });
            }
            resources.insert(sch_ptr.clone(), res);
        }

        if let Some(res) = resources.values_mut().find(|res| res.id == *base) {
            self.collect_anchors(sch, &sch_ptr, res, url)?;
        } else {
            debug_assert!(false, "base resource must exist");
        }

        for (&kw, &pos) in &self.subschemas {
            let Some(v) = obj.get(kw) else {
                continue;
            };
            if pos & POS_SELF != 0 {
                let ptr = sch_ptr.append(kw);
                self.collect_resources(v, base, ptr, url, resources)?;
            }
            if pos & POS_ITEM != 0
                && let Value::Array(arr) = v
            {
                for (i, item) in arr.iter().enumerate() {
                    let ptr = sch_ptr.append2(kw, &i.to_string());
                    self.collect_resources(item, base, ptr, url, resources)?;
                }
            }
            if pos & POS_PROP != 0
                && let Value::Object(obj) = v
            {
                for (pname, pvalue) in obj {
                    let ptr = sch_ptr.append2(kw, pname);
                    self.collect_resources(pvalue, base, ptr, url, resources)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn is_subschema(&self, ptr: &str) -> bool {
        if ptr.is_empty() {
            return true;
        }

        fn split(mut ptr: &str) -> (&str, &str) {
            ptr = &ptr[1..];
            if let Some(i) = ptr.find('/') {
                (&ptr[..i], &ptr[i..])
            } else {
                (ptr, "")
            }
        }

        let (tok, ptr) = split(ptr);

        if let Some(&pos) = self.subschemas.get(tok) {
            if pos & POS_SELF != 0 && self.is_subschema(ptr) {
                return true;
            }
            if !ptr.is_empty() {
                if pos & POS_PROP != 0 {
                    let (_, ptr) = split(ptr);
                    if self.is_subschema(ptr) {
                        return true;
                    }
                }
                if pos & POS_ITEM != 0 {
                    let (tok, ptr) = split(ptr);
                    if usize::from_str(tok).is_ok() && self.is_subschema(ptr) {
                        return true;
                    }
                }
            }
        }

        false
    }
}

fn load_std_metaschemas() -> Result<Schemas, CompileError> {
    let mut schemas = Schemas::new();
    let mut compiler = Compiler::new();
    compiler.enable_format_assertions();
    compiler.compile("https://json-schema.org/draft/2020-12/schema", &mut schemas)?;
    compiler.compile("https://json-schema.org/draft/2019-09/schema", &mut schemas)?;
    compiler.compile("http://json-schema.org/draft-07/schema", &mut schemas)?;
    compiler.compile("http://json-schema.org/draft-06/schema", &mut schemas)?;
    compiler.compile("http://json-schema.org/draft-04/schema", &mut schemas)?;
    Ok(schemas)
}

use std::{cmp::Ordering, collections::HashMap, error::Error, fmt::Display};

use crate::ecma::Regex;
use nanocode_url::Url;
use serde_json::{Map, Value};

use crate::{content::*, draft::*, ecma, formats::*, root::*, roots::*, util::*, *};

#[non_exhaustive]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Draft {
    V4,

    V6,

    V7,

    V2019_09,

    #[default]
    V2020_12,
}

impl Draft {
    pub fn from_url(url: &str) -> Option<Self> {
        match crate::draft::Draft::from_url(url) {
            Some(draft) => match draft.version {
                4 => Some(Self::V4),
                6 => Some(Self::V6),
                7 => Some(Self::V7),
                2019 => Some(Self::V2019_09),
                2020 => Some(Self::V2020_12),
                _ => None,
            },
            None => None,
        }
    }

    pub(crate) fn internal(&self) -> &'static crate::draft::Draft {
        match self {
            Self::V4 => &DRAFT4,
            Self::V6 => &DRAFT6,
            Self::V7 => &DRAFT7,
            Self::V2019_09 => &DRAFT2019,
            Self::V2020_12 => &DRAFT2020,
        }
    }
}

#[derive(Default)]
pub struct Compiler {
    roots: Roots,
    assert_format: bool,
    assert_content: bool,
    formats: HashMap<&'static str, Format>,
    decoders: HashMap<&'static str, Decoder>,
    media_types: HashMap<&'static str, MediaType>,
}

impl Compiler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_default_draft(&mut self, d: Draft) {
        self.roots.default_draft = d.internal()
    }

    pub const fn enable_format_assertions(&mut self) {
        self.assert_format = true;
    }

    pub const fn enable_content_assertions(&mut self) {
        self.assert_content = true;
    }

    pub fn use_loader(&mut self, url_loader: Box<dyn UrlLoader>) {
        self.roots.loader.use_loader(url_loader);
    }

    pub fn register_format(&mut self, format: Format) {
        if format.name != "regex" {
            self.formats.insert(format.name, format);
        }
    }

    pub fn register_content_encoding(&mut self, decoder: Decoder) {
        self.decoders.insert(decoder.name, decoder);
    }

    pub fn register_content_media_type(&mut self, media_type: MediaType) {
        self.media_types.insert(media_type.name, media_type);
    }

    pub fn add_resource(&mut self, loc: &str, json: Value) -> Result<(), CompileError> {
        let uf = UrlFrag::absolute(loc)?;
        self.roots.loader.add_doc(uf.url, json);
        Ok(())
    }

    pub fn compile(
        &mut self,
        loc: &str,
        target: &mut Schemas,
    ) -> Result<SchemaIndex, CompileError> {
        let uf = UrlFrag::absolute(loc)?;

        let up = self.roots.resolve_fragment(uf)?;

        let result = self.do_compile(up, target);
        if let Err(bug @ CompileError::Bug(_)) = &result {
            debug_assert!(false, "{bug}");
        }
        result
    }

    fn do_compile(
        &mut self,
        up: UrlPtr,
        target: &mut Schemas,
    ) -> Result<SchemaIndex, CompileError> {
        let mut queue = Queue::new();
        let mut compiled = Vec::new();

        let index = queue.enqueue_schema(target, up);
        if queue.schemas.is_empty() {
            return Ok(index);
        }

        while queue.schemas.len() > compiled.len() {
            let up = &queue.schemas[compiled.len()];
            self.roots.ensure_subschema(up)?;
            let Some(root) = self.roots.get(&up.url) else {
                return Err(CompileError::Bug("or_load didn't add".into()));
            };
            let doc = self.roots.loader.load(&root.url)?;
            let v = up.lookup(doc)?;
            let sch = self.compile_value(target, v, &up.clone(), root, &mut queue)?;
            compiled.push(sch);
            self.roots.insert(&mut queue.roots);
        }

        target.insert(queue.schemas, compiled);
        Ok(index)
    }

    fn compile_value(
        &self,
        schemas: &Schemas,
        v: &Value,
        up: &UrlPtr,
        root: &Root,
        queue: &mut Queue,
    ) -> Result<Schema, CompileError> {
        let mut s = Schema::new(up.to_string());
        s.draft_version = root.draft.version;

        let len = queue.schemas.len();
        s.idx = queue.enqueue_schema(schemas, up.to_owned());
        debug_assert_eq!(queue.schemas.len(), len, "{up} should already be in queue");

        s.resource = {
            let base = UrlPtr {
                url: up.url.clone(),
                ptr: root.resource(&up.ptr).ptr.clone(),
            };
            queue.enqueue_schema(schemas, base)
        };

        if s.idx == s.resource && root.draft.version >= 2020 {
            let res = root.resource(&up.ptr);
            for (anchor, anchor_ptr) in &res.anchors {
                if res.dynamic_anchors.contains(anchor) {
                    let up = UrlPtr {
                        url: up.url.clone(),
                        ptr: anchor_ptr.clone(),
                    };
                    let danchor_sch = queue.enqueue_schema(schemas, up);
                    s.dynamic_anchors.insert(anchor.to_string(), danchor_sch);
                }
            }
        }

        match v {
            Value::Object(obj) => {
                if obj.is_empty() {
                    s.boolean = Some(true);
                } else {
                    ObjCompiler {
                        c: self,
                        obj,
                        up,
                        schemas,
                        root,
                        queue,
                    }
                    .compile_obj(&mut s)?;
                }
            }
            Value::Bool(b) => s.boolean = Some(*b),
            _ => {}
        }

        s.all_props_evaluated = s.additional_properties.is_some();
        s.all_items_evaluated = if s.draft_version < 2020 {
            s.additional_items.is_some() || matches!(s.items, Some(Items::SchemaRef(_)))
        } else {
            s.items2020.is_some()
        };
        s.num_items_evaluated = if let Some(Items::SchemaRefs(list)) = &s.items {
            list.len()
        } else {
            s.prefix_items.len()
        };

        Ok(s)
    }
}

struct ObjCompiler<'c, 'v, 'l, 's, 'r, 'q> {
    c: &'c Compiler,
    obj: &'v Map<String, Value>,
    up: &'l UrlPtr,
    schemas: &'s Schemas,
    root: &'r Root,
    queue: &'q mut Queue,
}

impl ObjCompiler<'_, '_, '_, '_, '_, '_> {
    fn compile_obj(&mut self, s: &mut Schema) -> Result<(), CompileError> {
        self.compile_draft4(s)?;
        if self.draft_version() >= 6 {
            self.compile_draft6(s)?;
        }
        if self.draft_version() >= 7 {
            self.compile_draft7(s)?;
        }
        if self.draft_version() >= 2019 {
            self.compile_draft2019(s)?;
        }
        if self.draft_version() >= 2020 {
            self.compile_draft2020(s)?;
        }
        Ok(())
    }

    fn compile_draft4(&mut self, s: &mut Schema) -> Result<(), CompileError> {
        if self.has_vocab("core") {
            s.ref_ = self.enqueue_ref("$ref")?;
            if s.ref_.is_some() && self.draft_version() < 2019 {
                return Ok(());
            }
        }

        if self.has_vocab("applicator") {
            s.all_of = self.enqueue_arr("allOf");
            s.any_of = self.enqueue_arr("anyOf");
            s.one_of = self.enqueue_arr("oneOf");
            s.not = self.enqueue_prop("not");

            if self.draft_version() < 2020 {
                match self.value("items") {
                    Some(Value::Array(_)) => {
                        s.items = Some(Items::SchemaRefs(self.enqueue_arr("items")));
                        s.additional_items = self.enquue_additional("additionalItems");
                    }
                    _ => s.items = self.enqueue_prop("items").map(Items::SchemaRef),
                }
            }

            s.properties = self.enqueue_map("properties");
            s.pattern_properties = {
                let mut v = vec![];
                if let Some(Value::Object(obj)) = self.value("patternProperties") {
                    for pname in obj.keys() {
                        let ecma =
                            ecma::convert(pname).map_err(|src| CompileError::InvalidRegex {
                                url: self.up.format("patternProperties"),
                                regex: pname.to_owned(),
                                src,
                            })?;
                        let regex =
                            Regex::new(ecma.as_ref()).map_err(|e| CompileError::InvalidRegex {
                                url: self.up.format("patternProperties"),
                                regex: ecma.into_owned(),
                                src: e.into(),
                            })?;
                        let ptr = self.up.ptr.append2("patternProperties", pname);
                        let sch = self.enqueue_schema(ptr);
                        v.push((regex, sch));
                    }
                }
                v
            };

            s.additional_properties = self.enquue_additional("additionalProperties");

            if let Some(Value::Object(deps)) = self.value("dependencies") {
                s.dependencies = deps
                    .iter()
                    .filter_map(|(k, v)| {
                        let v = match v {
                            Value::Array(_) => Some(Dependency::Props(to_strings(v))),
                            _ => {
                                let ptr = self.up.ptr.append2("dependencies", k);
                                Some(Dependency::SchemaRef(self.enqueue_schema(ptr)))
                            }
                        };
                        v.map(|v| (k.clone(), v))
                    })
                    .collect();
            }
        }

        if self.has_vocab("validation") {
            match self.value("type") {
                Some(Value::String(t)) => {
                    if let Some(t) = Type::from_str(t) {
                        s.types.add(t)
                    }
                }
                Some(Value::Array(arr)) => {
                    for t in arr {
                        if let Value::String(t) = t
                            && let Some(t) = Type::from_str(t)
                        {
                            s.types.add(t)
                        }
                    }
                }
                _ => {}
            }

            if let Some(Value::Array(e)) = self.value("enum") {
                let mut types = Types::default();
                for item in e {
                    types.add(Type::of(item));
                }
                s.enum_ = Some(Enum {
                    types,
                    values: e.clone(),
                });
            }

            s.multiple_of = self.num("multipleOf");

            s.maximum = self.num("maximum");
            if let Some(Value::Bool(exclusive)) = self.value("exclusiveMaximum") {
                if *exclusive {
                    s.exclusive_maximum = s.maximum.take();
                }
            } else {
                s.exclusive_maximum = self.num("exclusiveMaximum");
            }

            s.minimum = self.num("minimum");
            if let Some(Value::Bool(exclusive)) = self.value("exclusiveMinimum") {
                if *exclusive {
                    s.exclusive_minimum = s.minimum.take();
                }
            } else {
                s.exclusive_minimum = self.num("exclusiveMinimum");
            }

            s.max_length = self.num("maxLength");
            s.min_length = self.num("minLength");

            if let Some(Value::String(p)) = self.value("pattern") {
                let p = ecma::convert(p).map_err(CompileError::Bug)?;
                s.pattern = Some(Regex::new(p.as_ref()).map_err(|e| CompileError::Bug(e.into()))?);
            }

            s.max_items = self.num("maxItems");
            s.min_items = self.num("minItems");
            s.unique_items = self.bool("uniqueItems");

            s.max_properties = self.num("maxProperties");
            s.min_properties = self.num("minProperties");

            if let Some(req) = self.value("required") {
                s.required = to_strings(req);
            }
        }

        if (self.c.assert_format
            || self.has_vocab(match self.draft_version().cmp(&2019) {
                Ordering::Less => "core",
                Ordering::Equal => "format",
                Ordering::Greater => "format-assertion",
            }))
            && let Some(Value::String(format)) = self.value("format")
        {
            if format == "hostname" && self.draft_version() < 7 {
                s.format = Some(Format {
                    name: "hostname",
                    func: formats::validate_legacy_hostname,
                });
                return Ok(());
            }
            s.format = self
                .c
                .formats
                .get(format.as_str())
                .or_else(|| FORMATS.get(format.as_str()))
                .cloned();
        }

        Ok(())
    }

    fn compile_draft6(&mut self, s: &mut Schema) -> Result<(), CompileError> {
        if self.has_vocab("applicator") {
            s.contains = self.enqueue_prop("contains");
            s.property_names = self.enqueue_prop("propertyNames");
        }

        if self.has_vocab("validation") {
            s.constant = self.value("const").cloned();
        }

        Ok(())
    }

    fn compile_draft7(&mut self, s: &mut Schema) -> Result<(), CompileError> {
        if self.has_vocab("applicator") {
            s.if_ = self.enqueue_prop("if");
            if s.if_.is_some() {
                if !self.bool_schema("if", false) {
                    s.then = self.enqueue_prop("then");
                }
                if !self.bool_schema("if", true) {
                    s.else_ = self.enqueue_prop("else");
                }
            }
        }

        if self.c.assert_content || self.draft_version() < 2019 {
            if let Some(Value::String(encoding)) = self.value("contentEncoding") {
                s.content_encoding = self
                    .c
                    .decoders
                    .get(encoding.as_str())
                    .or_else(|| DECODERS.get(encoding.as_str()))
                    .cloned();
            }

            if let Some(Value::String(media_type)) = self.value("contentMediaType") {
                s.content_media_type = self
                    .c
                    .media_types
                    .get(media_type.as_str())
                    .or_else(|| MEDIA_TYPES.get(media_type.as_str()))
                    .cloned();
            }
        }

        Ok(())
    }

    fn compile_draft2019(&mut self, s: &mut Schema) -> Result<(), CompileError> {
        if self.has_vocab("core") {
            s.recursive_ref = self.enqueue_ref("$recursiveRef")?;
            s.recursive_anchor = self.bool("$recursiveAnchor");
        }

        if self.has_vocab("validation") {
            if s.contains.is_some() {
                s.max_contains = self.num("maxContains");
                s.min_contains = self.num("minContains");
            }

            if let Some(Value::Object(dep_req)) = self.value("dependentRequired") {
                for (pname, pvalue) in dep_req {
                    s.dependent_required
                        .push((pname.clone(), to_strings(pvalue)));
                }
            }
        }

        if self.has_vocab("applicator") {
            s.dependent_schemas = self.enqueue_map("dependentSchemas");
        }

        if self.has_vocab(match self.draft_version() {
            2019 => "applicator",
            _ => "unevaluated",
        }) {
            s.unevaluated_items = self.enqueue_prop("unevaluatedItems");
            s.unevaluated_properties = self.enqueue_prop("unevaluatedProperties");
        }

        if self.c.assert_content
            && s.content_media_type
                .map(|mt| mt.json_compatible)
                .unwrap_or(false)
        {
            s.content_schema = self.enqueue_prop("contentSchema");
        }

        Ok(())
    }

    fn compile_draft2020(&mut self, s: &mut Schema) -> Result<(), CompileError> {
        if self.has_vocab("core") {
            if let Some(sch) = self.enqueue_ref("$dynamicRef")?
                && let Some(Value::String(dref)) = self.value("$dynamicRef")
            {
                let Ok((_, frag)) = Fragment::split(dref) else {
                    let loc = self.up.format("$dynamicRef");
                    return Err(CompileError::ParseAnchorError { loc });
                };
                let anchor = match frag {
                    Fragment::Anchor(Anchor(s)) => Some(s),
                    Fragment::JsonPointer(_) => None,
                };
                s.dynamic_ref = Some(DynamicRef { sch, anchor });
            };

            if let Some(Value::String(anchor)) = self.value("$dynamicAnchor") {
                s.dynamic_anchor = Some(anchor.to_owned());
            }
        }

        if self.has_vocab("applicator") {
            s.prefix_items = self.enqueue_arr("prefixItems");
            s.items2020 = self.enqueue_prop("items");
        }

        Ok(())
    }
}

impl ObjCompiler<'_, '_, '_, '_, '_, '_> {
    fn enqueue_schema(&mut self, ptr: JsonPointer) -> SchemaIndex {
        let up = UrlPtr {
            url: self.up.url.clone(),
            ptr,
        };
        self.queue.enqueue_schema(self.schemas, up)
    }

    fn enqueue_prop(&mut self, pname: &'static str) -> Option<SchemaIndex> {
        if self.obj.contains_key(pname) {
            let ptr = self.up.ptr.append(pname);
            Some(self.enqueue_schema(ptr))
        } else {
            None
        }
    }

    fn enqueue_arr(&mut self, pname: &'static str) -> Vec<SchemaIndex> {
        if let Some(Value::Array(arr)) = self.obj.get(pname) {
            (0..arr.len())
                .map(|i| {
                    let ptr = self.up.ptr.append2(pname, &i.to_string());
                    self.enqueue_schema(ptr)
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    fn enqueue_map<T>(&mut self, pname: &'static str) -> T
    where
        T: Default,
        T: FromIterator<(String, SchemaIndex)>,
    {
        if let Some(Value::Object(obj)) = self.obj.get(pname) {
            obj.keys()
                .map(|k| {
                    let ptr = self.up.ptr.append2(pname, k);
                    (k.clone(), self.enqueue_schema(ptr))
                })
                .collect()
        } else {
            T::default()
        }
    }

    fn enqueue_ref(&mut self, pname: &str) -> Result<Option<SchemaIndex>, CompileError> {
        let Some(Value::String(ref_)) = self.obj.get(pname) else {
            return Ok(None);
        };
        let base_url = self.root.base_url(&self.up.ptr);
        let abs_ref = UrlFrag::join(base_url, ref_)?;
        if let Some(resolved_ref) = self.root.resolve(&abs_ref)? {
            return Ok(Some(self.enqueue_schema(resolved_ref.ptr)));
        }

        let up = self.queue.resolve_anchor(abs_ref, &self.c.roots)?;
        Ok(Some(self.queue.enqueue_schema(self.schemas, up)))
    }

    fn enquue_additional(&mut self, pname: &'static str) -> Option<Additional> {
        if let Some(Value::Bool(b)) = self.obj.get(pname) {
            Some(Additional::Bool(*b))
        } else {
            self.enqueue_prop(pname).map(Additional::SchemaRef)
        }
    }
}

impl<'v> ObjCompiler<'_, 'v, '_, '_, '_, '_> {
    const fn draft_version(&self) -> usize {
        self.root.draft.version
    }

    fn has_vocab(&self, name: &str) -> bool {
        self.root.has_vocab(name)
    }

    fn value(&self, pname: &str) -> Option<&'v Value> {
        self.obj.get(pname)
    }

    fn bool(&self, pname: &str) -> bool {
        matches!(self.obj.get(pname), Some(Value::Bool(true)))
    }

    fn num(&self, pname: &str) -> Option<Number> {
        if let Some(Value::Number(n)) = self.obj.get(pname) {
            Some(n.clone())
        } else {
            None
        }
    }

    fn bool_schema(&self, pname: &str, b: bool) -> bool {
        if let Some(Value::Bool(v)) = self.obj.get(pname) {
            return *v == b;
        }
        false
    }
}

#[derive(Debug)]
pub enum CompileError {
    ParseUrlError {
        url: String,
        src: Box<dyn Error>,
    },

    LoadUrlError {
        url: String,
        src: Box<dyn Error>,
    },

    UnsupportedUrlScheme {
        url: String,
    },

    InvalidMetaSchemaUrl {
        url: String,
        src: Box<dyn Error>,
    },

    UnsupportedDraft {
        url: String,
    },

    MetaSchemaCycle {
        url: String,
    },

    ValidationError {
        url: String,
        src: ValidationError<'static, 'static>,
    },

    ParseIdError {
        loc: String,
    },

    ParseAnchorError {
        loc: String,
    },

    DuplicateId {
        url: String,
        id: String,
        ptr1: String,
        ptr2: String,
    },

    DuplicateAnchor {
        anchor: String,
        url: String,
        ptr1: String,
        ptr2: String,
    },

    InvalidJsonPointer(String),

    JsonPointerNotFound(String),

    AnchorNotFound {
        url: String,
        reference: String,
    },

    UnsupportedVocabulary {
        url: String,
        vocabulary: String,
    },

    InvalidRegex {
        url: String,
        regex: String,
        src: Box<dyn Error>,
    },

    Bug(Box<dyn Error>),
}

impl Error for CompileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ParseUrlError { src, .. } => Some(src.as_ref()),
            Self::LoadUrlError { src, .. } => Some(src.as_ref()),
            Self::InvalidMetaSchemaUrl { src, .. } => Some(src.as_ref()),
            Self::ValidationError { src, .. } => Some(src),
            Self::Bug(src) => Some(src.as_ref()),
            _ => None,
        }
    }
}

impl Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParseUrlError { url, src } => {
                if f.alternate() {
                    write!(f, "error parsing url {url}: {src}")
                } else {
                    write!(f, "error parsing {url}")
                }
            }
            Self::LoadUrlError { url, src } => {
                if f.alternate() {
                    write!(f, "error loading {url}: {src}")
                } else {
                    write!(f, "error loading {url}")
                }
            }
            Self::UnsupportedUrlScheme { url } => write!(f, "unsupported scheme in {url}"),
            Self::InvalidMetaSchemaUrl { url, src } => {
                if f.alternate() {
                    write!(f, "invalid $schema in {url}: {src}")
                } else {
                    write!(f, "invalid $schema in {url}")
                }
            }
            Self::UnsupportedDraft { url } => write!(f, "draft {url} is not supported"),
            Self::MetaSchemaCycle { url } => {
                write!(f, "cycle in resolving $schema in {url}")
            }
            Self::ValidationError { url, src } => {
                if f.alternate() {
                    write!(f, "{url} is not valid against metaschema: {src}")
                } else {
                    write!(f, "{url} is not valid against metaschema")
                }
            }
            Self::ParseIdError { loc } => write!(f, "error in parsing id at {loc}"),
            Self::ParseAnchorError { loc } => write!(f, "error in parsing anchor at {loc}"),
            Self::DuplicateId {
                url,
                id,
                ptr1,
                ptr2,
            } => write!(f, "duplicate $id {id} in {url} at {ptr1:?} and {ptr2:?}"),
            Self::DuplicateAnchor {
                anchor,
                url,
                ptr1,
                ptr2,
            } => {
                write!(
                    f,
                    "duplicate anchor {anchor:?} in {url} at {ptr1:?} and {ptr2:?}"
                )
            }
            Self::InvalidJsonPointer(loc) => write!(f, "invalid json-pointer {loc}"),
            Self::JsonPointerNotFound(loc) => write!(f, "json-pointer in {loc} not found"),
            Self::AnchorNotFound { url, reference } => {
                write!(
                    f,
                    "anchor in reference {reference} is not found in schema {url}"
                )
            }
            Self::UnsupportedVocabulary { url, vocabulary } => {
                write!(f, "unsupported vocabulary {vocabulary} in {url}")
            }
            Self::InvalidRegex { url, regex, src } => {
                if f.alternate() {
                    write!(f, "invalid regex {} at {url}: {src}", quote(regex))
                } else {
                    write!(f, "invalid regex {} at {url}", quote(regex))
                }
            }
            Self::Bug(src) => {
                write!(
                    f,
                    "encountered bug in jsonschema compiler. please report: {src}"
                )
            }
        }
    }
}

fn to_strings(v: &Value) -> Vec<String> {
    if let Value::Array(a) = v {
        a.iter()
            .filter_map(|t| {
                if let Value::String(t) = t {
                    Some(t.clone())
                } else {
                    None
                }
            })
            .collect()
    } else {
        vec![]
    }
}

pub(crate) struct Queue {
    pub(crate) schemas: Vec<UrlPtr>,
    pub(crate) roots: HashMap<Url, Root>,
}

impl Queue {
    fn new() -> Self {
        Self {
            schemas: vec![],
            roots: HashMap::new(),
        }
    }

    pub(crate) fn resolve_anchor(
        &mut self,
        uf: UrlFrag,
        roots: &Roots,
    ) -> Result<UrlPtr, CompileError> {
        match uf.frag {
            Fragment::JsonPointer(ptr) => Ok(UrlPtr { url: uf.url, ptr }),
            Fragment::Anchor(_) => {
                let root = match roots.get(&uf.url).or_else(|| self.roots.get(&uf.url)) {
                    Some(root) => root,
                    None => {
                        let doc = roots.loader.load(&uf.url)?;
                        let r = roots.create_root(uf.url.clone(), doc)?;
                        self.roots.entry(uf.url).or_insert(r)
                    }
                };
                root.resolve_fragment(&uf.frag)
            }
        }
    }

    pub(crate) fn enqueue_schema(&mut self, schemas: &Schemas, up: UrlPtr) -> SchemaIndex {
        if let Some(sch) = schemas.get_by_loc(&up) {
            return sch.idx;
        }
        if let Some(qindex) = self.schemas.iter().position(|e| *e == up) {
            return SchemaIndex(schemas.size() + qindex);
        }

        self.schemas.push(up);
        SchemaIndex(schemas.size() + self.schemas.len() - 1)
    }
}

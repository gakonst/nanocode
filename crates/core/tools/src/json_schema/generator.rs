use super::{JsonSchema, Schema};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

const DEFINITIONS_PATH: &str = "#/definitions/";
const META_SCHEMA_DRAFT_07_URL: &str = "http://json-schema.org/draft-07/schema#";
const META_SCHEMA_DRAFT_2019_09_URL: &str = "https://json-schema.org/draft/2019-09/schema";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draft {
    Draft07,
    Draft2019_09,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaSettings {
    pub draft: Draft,
    pub inline_subschemas: bool,
    pub option_add_null_type: bool,
}

impl SchemaSettings {
    #[must_use]
    pub const fn draft07() -> Self {
        Self {
            draft: Draft::Draft07,
            inline_subschemas: false,
            option_add_null_type: true,
        }
    }

    #[must_use]
    pub const fn draft2019_09() -> Self {
        Self {
            draft: Draft::Draft2019_09,
            inline_subschemas: false,
            option_add_null_type: true,
        }
    }

    #[must_use]
    pub fn root_schema_for<T: ?Sized + JsonSchema>(self) -> Map<String, Value> {
        SchemaGenerator::new(self).into_root_schema_for::<T>()
    }
}

#[derive(Debug)]
pub struct SchemaGenerator {
    settings: SchemaSettings,
    definitions: BTreeMap<String, Schema>,
    pending_schema_ids: BTreeSet<Cow<'static, str>>,
    schema_id_to_name: BTreeMap<Cow<'static, str>, String>,
    used_schema_names: BTreeSet<String>,
}

impl SchemaGenerator {
    #[must_use]
    pub const fn new(settings: SchemaSettings) -> Self {
        Self {
            settings,
            definitions: BTreeMap::new(),
            pending_schema_ids: BTreeSet::new(),
            schema_id_to_name: BTreeMap::new(),
            used_schema_names: BTreeSet::new(),
        }
    }

    #[must_use]
    pub const fn settings(&self) -> &SchemaSettings {
        &self.settings
    }

    pub fn subschema_for<T: ?Sized + JsonSchema>(&mut self) -> Schema {
        let id = T::schema_id();
        let referenced = T::is_referenceable()
            && (!self.settings.inline_subschemas || self.pending_schema_ids.contains(&id));
        if !referenced {
            return self.json_schema_internal::<T>(id);
        }
        let name = match self.schema_id_to_name.get(&id) {
            Some(name) => name.clone(),
            None => {
                let name = self.unused_name(T::schema_name());
                self.used_schema_names.insert(name.clone());
                self.schema_id_to_name.insert(id.clone(), name.clone());
                name
            }
        };
        let reference = format!("{DEFINITIONS_PATH}{name}");
        if !self.definitions.contains_key(&name) {
            self.definitions.insert(name.clone(), Schema::Bool(false));
            let schema = self.json_schema_internal::<T>(id);
            self.definitions.insert(name, schema);
        }
        Schema::new_ref(reference)
    }

    fn unused_name(&self, base_name: String) -> String {
        if !self.used_schema_names.contains(&base_name) {
            return base_name;
        }
        let candidates_max = self.used_schema_names.len() + 2;
        (2..=candidates_max)
            .map(|suffix| format!("{base_name}{suffix}"))
            .find(|name| !self.used_schema_names.contains(name))
            .expect("one of used_schema_names.len() + 1 suffixed names is unused")
    }

    fn json_schema_internal<T: ?Sized + JsonSchema>(&mut self, id: Cow<'static, str>) -> Schema {
        let added = self.pending_schema_ids.insert(id.clone());
        let schema = T::json_schema(self);
        if added {
            self.pending_schema_ids.remove(&id);
        }
        schema
    }

    #[must_use]
    pub fn into_root_schema_for<T: ?Sized + JsonSchema>(mut self) -> Map<String, Value> {
        let mut schema = self.json_schema_internal::<T>(T::schema_id()).into_object();
        schema.metadata().title.get_or_insert_with(T::schema_name);
        let meta_schema = match self.settings.draft {
            Draft::Draft07 => {
                schema.remove_ref_siblings();
                for definition in self.definitions.values_mut() {
                    if let Schema::Object(object) = definition {
                        object.remove_ref_siblings();
                    }
                }
                META_SCHEMA_DRAFT_07_URL
            }
            Draft::Draft2019_09 => META_SCHEMA_DRAFT_2019_09_URL,
        };
        let mut root = Map::new();
        root.insert("$schema".to_owned(), Value::String(meta_schema.to_owned()));
        schema.write(&mut root);
        if !self.definitions.is_empty() {
            root.insert(
                "definitions".to_owned(),
                Value::Object(
                    self.definitions
                        .iter()
                        .map(|(name, schema)| (name.clone(), schema.to_value()))
                        .collect(),
                ),
            );
        }
        root
    }
}

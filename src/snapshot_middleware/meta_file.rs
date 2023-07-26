use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

use anyhow::{format_err, Context};

use rbx_dom_weak::{
    types::{Attributes, Variant},
    Instance,
};
use serde::{Deserialize, Serialize};

use crate::{
    resolution::UnresolvedValue,
    snapshot::{InstanceSnapshot, PropertiesFiltered, PropertyFilter, ToVariantBinaryString},
};

/// Represents metadata in a sibling file with the same basename.
///
/// As an example, hello.meta.json next to hello.lua would allow assigning
/// additional metadata to the instance resulting from hello.lua.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_unknown_instances: Option<bool>,

    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub properties: HashMap<String, UnresolvedValue>,

    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, UnresolvedValue>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,

    #[serde(skip)]
    pub path: PathBuf,
}

impl MetadataFile {
    pub fn from_slice(slice: &[u8], path: PathBuf) -> anyhow::Result<Self> {
        let mut meta: Self = serde_json::from_slice(slice).with_context(|| {
            format!(
                "File contained malformed .meta.json data: {}",
                path.display()
            )
        })?;

        meta.path = path;
        Ok(meta)
    }

    pub fn with_instance_props(
        self,
        instance: &Instance,
        skip_props: HashSet<&str>,
        filters: &BTreeMap<String, PropertyFilter>,
    ) -> Self {
        if instance.name == "ModuleScript" {
            log::trace!("Skipping properties for ModuleScript: {:#?}", skip_props);
        }

        Self {
            properties: HashMap::from_iter(instance.properties_filtered(filters, true).filter_map(
                |(k, v)| {
                    if k == "Attributes" {
                        return None;
                    }

                    let mut v = v.clone();

                    if let Variant::SharedString(_) = v {
                        log::trace!(
                            "Converting {}.{} from SharedString to BinaryString",
                            instance.class,
                            k
                        );
                        v = v.to_variant_binary_string().unwrap();
                    }

                    if !skip_props.contains(k) {
                        Some((
                            k.to_string(),
                            UnresolvedValue::from_variant_property(&instance.class, k, v),
                        ))
                    } else {
                        None
                    }
                },
            )),
            attributes: instance.properties.get("Attributes").map_or_else(
                || HashMap::new(),
                |attributes| {
                    match attributes {
                        Variant::Attributes(attributes) => attributes
                            .iter()
                            .map(|(k, v)| (k.clone(), UnresolvedValue::from_variant(v.clone())))
                            .collect(),
                        _ => HashMap::new(), // TODO: error here?
                    }
                },
            ),
            ..self
        }
    }

    pub fn with_path(self, path: PathBuf) -> Self {
        Self { path, ..self }
    }

    pub fn minimize_diff(&mut self, prev_meta_file: &Self, base_class: Option<&str>) -> () {
        self.properties
            .iter()
            .filter_map(|(key, value)| {
                if self.resolve_property(key, base_class)
                    == prev_meta_file.resolve_property(key, base_class)
                {
                    Some((key.to_string(), value.clone()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .for_each(|(key, value)| {
                self.properties.insert(key, value);
            });

        self.attributes
            .iter()
            .filter_map(|(key, value)| {
                if self.resolve_property(key, base_class)
                    == prev_meta_file.resolve_property(key, base_class)
                {
                    Some((key.to_string(), value.clone()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .for_each(|(key, value)| {
                self.attributes.insert(key, value);
            });
    }

    pub fn resolve_property(&self, key: &str, base_class: Option<&str>) -> Option<Variant> {
        let class_name = self.class_name.as_ref().map(|v| v.as_str()).or(base_class);
        self.properties
            .get(key)
            .map(|unresolved_value| match class_name {
                Some(class_name) => unresolved_value.clone().resolve(class_name, key).ok(),
                None => unresolved_value.clone().resolve_unambiguous().ok(),
            })
            .flatten()
    }

    pub fn resolve_attribute(&self, key: &str) -> Option<Variant> {
        self.attributes
            .get(key)
            .map(|unresolved_value| unresolved_value.clone().resolve_unambiguous().ok())
            .flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
            && self.attributes.is_empty()
            && self.class_name.is_none()
            && self.ignore_unknown_instances.is_none()
    }

    pub fn apply_ignore_unknown_instances(&mut self, snapshot: &mut InstanceSnapshot) {
        if let Some(ignore) = self.ignore_unknown_instances.take() {
            snapshot.metadata.ignore_unknown_instances = ignore;
        }
    }

    pub fn apply_properties(&mut self, snapshot: &mut InstanceSnapshot) -> anyhow::Result<()> {
        let path = &self.path;

        for (key, unresolved) in self.properties.drain() {
            let value = unresolved
                .resolve(&snapshot.class_name, &key)
                .with_context(|| format!("error applying meta file {}", path.display()))?;

            snapshot.properties.insert(key, value);
        }

        if !self.attributes.is_empty() {
            let mut attributes = Attributes::new();

            for (key, unresolved) in self.attributes.drain() {
                let value = unresolved.resolve_unambiguous()?;
                attributes.insert(key, value);
            }

            snapshot
                .properties
                .insert("Attributes".into(), attributes.into());
        }

        Ok(())
    }

    fn apply_class_name(&mut self, snapshot: &mut InstanceSnapshot) -> anyhow::Result<()> {
        if let Some(class_name) = self.class_name.take() {
            if snapshot.class_name != "Folder" {
                // TODO: Turn into error type
                return Err(format_err!(
                    "className in init.meta.json can only be specified if the \
                     affected directory would turn into a Folder instance."
                ));
            }

            snapshot.class_name = Cow::Owned(class_name);
        }

        Ok(())
    }

    pub fn apply_all(&mut self, snapshot: &mut InstanceSnapshot) -> anyhow::Result<()> {
        self.apply_ignore_unknown_instances(snapshot);
        // We must apply class name before properties because property decoding
        // may depend on class name.
        self.apply_class_name(snapshot)?;
        self.apply_properties(snapshot)?;
        Ok(())
    }

    // TODO: Add method to allow selectively applying parts of metadata and
    // throwing errors if invalid parts are specified.
}

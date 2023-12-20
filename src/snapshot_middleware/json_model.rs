use std::{borrow::Cow, collections::HashMap, path::Path, str};

use anyhow::Context;
use memofs::Vfs;
use rbx_dom_weak::types::{Attributes, Ref, Variant};
use serde::{Deserialize, Serialize};

use crate::{
    resolution::UnresolvedValue,
    snapshot::{InstanceContext, InstanceSnapshot},
    syncback::{is_valid_file_name, FsSnapshot, SyncbackReturn, SyncbackSnapshot},
    variant_eq::variant_eq,
};

pub fn snapshot_json_model(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    name: &str,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let contents = vfs.read(path)?;
    let contents_str = str::from_utf8(&contents)
        .with_context(|| format!("File was not valid UTF-8: {}", path.display()))?;

    if contents_str.trim().is_empty() {
        return Ok(None);
    }

    let mut instance: JsonModel = serde_json::from_str(contents_str)
        .with_context(|| format!("File is not a valid JSON model: {}", path.display()))?;

    if let Some(top_level_name) = &instance.name {
        let new_name = format!("{}.model.json", top_level_name);

        log::warn!(
            "Model at path {} had a top-level Name field. \
            This field has been ignored since Rojo 6.0.\n\
            Consider removing this field and renaming the file to {}.",
            new_name,
            path.display()
        );
    }

    instance.name = Some(name.to_owned());

    let mut snapshot = instance
        .into_snapshot()
        .with_context(|| format!("Could not load JSON model: {}", path.display()))?;

    snapshot.metadata = snapshot
        .metadata
        .instigating_source(path)
        .relevant_paths(vec![path.to_path_buf()])
        .context(context);

    Ok(Some(snapshot))
}

pub fn syncback_json_model<'new, 'old>(
    snapshot: &SyncbackSnapshot<'new, 'old>,
) -> anyhow::Result<SyncbackReturn<'new, 'old>> {
    if !is_valid_file_name(&snapshot.name) {
        anyhow::bail!("cannot create a file with name {}", snapshot.name);
    }

    let mut path = snapshot.parent_path.join(&snapshot.name);
    path.set_extension("model.json");

    let new_inst = snapshot.new_inst();
    let mut model = JsonModel::new(&new_inst.name, &new_inst.class);
    let mut properties = HashMap::with_capacity(new_inst.properties.len());

    let class_data = rbx_reflection_database::get()
        .classes
        .get(model.class_name.as_str());

    // TODO handle attributes separately
    if let Some(old_inst) = snapshot.old_inst() {
        for (name, value) in &new_inst.properties {
            // We do not currently support Ref properties.
            if matches!(value, Variant::Ref(_)) {
                continue;
            }
            if old_inst.properties().contains_key(name) {
                properties.insert(name.clone(), UnresolvedValue::from(value.clone()));
            }
        }
    } else {
        if let Some(class_data) = class_data {
            let default_properties = &class_data.default_properties;
            for (name, value) in new_inst.properties.clone() {
                // We do not currently support Ref properties.
                if matches!(value, Variant::Ref(_)) {
                    continue;
                }
                match default_properties.get(name.as_str()) {
                    Some(default) if variant_eq(&value, default) => {}
                    _ => {
                        properties.insert(name, UnresolvedValue::from(value));
                    }
                }
            }
        } else {
            for (name, value) in new_inst.properties.clone() {
                if matches!(value, Variant::Ref(_)) {
                    continue;
                }
                properties.insert(name, UnresolvedValue::from(value));
            }
        }
    }
    model.set_properties(properties);

    // TODO children

    Ok(SyncbackReturn {
        inst_snapshot: InstanceSnapshot::from_instance(new_inst),
        fs_snapshot: FsSnapshot::new().with_file(
            &path,
            serde_json::to_vec_pretty(&model).context("failed to serialize new JSON Model")?,
        ),
        children: Vec::new(),
        removed_children: Vec::new(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonModel {
    #[serde(alias = "Name", skip_serializing)]
    name: Option<String>,

    #[serde(alias = "ClassName")]
    class_name: String,

    #[serde(
        alias = "Children",
        default = "Vec::new",
        skip_serializing_if = "Vec::is_empty"
    )]
    children: Vec<JsonModel>,

    #[serde(
        alias = "Properties",
        default = "HashMap::new",
        skip_serializing_if = "HashMap::is_empty"
    )]
    properties: HashMap<String, UnresolvedValue>,

    #[serde(default = "HashMap::new", skip_serializing_if = "HashMap::is_empty")]
    attributes: HashMap<String, UnresolvedValue>,
}

impl JsonModel {
    fn into_snapshot(self) -> anyhow::Result<InstanceSnapshot> {
        let name = self.name.unwrap_or_else(|| self.class_name.clone());
        let class_name = self.class_name;

        let mut children = Vec::with_capacity(self.children.len());
        for child in self.children {
            children.push(child.into_snapshot()?);
        }

        let mut properties = HashMap::with_capacity(self.properties.len());
        for (key, unresolved) in self.properties {
            let value = unresolved.resolve(&class_name, &key)?;
            properties.insert(key, value);
        }

        if !self.attributes.is_empty() {
            let mut attributes = Attributes::new();

            for (key, unresolved) in self.attributes {
                let value = unresolved.resolve_unambiguous()?;
                attributes.insert(key, value);
            }

            properties.insert("Attributes".into(), attributes.into());
        }

        Ok(InstanceSnapshot {
            snapshot_id: Ref::none(),
            metadata: Default::default(),
            name: Cow::Owned(name),
            class_name: Cow::Owned(class_name),
            properties,
            children,
        })
    }

    /// Constructs an empty JSON model with the provided name and class.
    #[inline]
    pub fn new(name: &str, class_name: &str) -> Self {
        Self {
            name: Some(name.to_string()),
            class_name: class_name.to_string(),
            children: Vec::new(),
            properties: HashMap::new(),
            attributes: HashMap::new(),
        }
    }

    /// Sets the properties of this `JsonModel`.
    #[inline]
    pub fn set_properties(&mut self, properties: HashMap<String, UnresolvedValue>) {
        self.properties = properties;
    }

    /// Sets the attributes of this `JsonModel`.
    #[inline]
    pub fn set_attributes(&mut self, attributes: HashMap<String, UnresolvedValue>) {
        self.attributes = attributes;
    }

    /// Pushes the provided `JsonModel` as a child of this one.
    #[inline]
    pub fn push_child(&mut self, child: Self) {
        self.children.push(child);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use memofs::{InMemoryFs, VfsSnapshot};

    #[test]
    fn model_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.model.json",
            VfsSnapshot::file(
                r#"
                    {
                      "className": "IntValue",
                      "properties": {
                        "Value": 5
                      },
                      "children": [
                        {
                          "name": "The Child",
                          "className": "StringValue"
                        }
                      ]
                    }
                "#,
            ),
        )
        .unwrap();

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_json_model(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo.model.json"),
            "foo",
        )
        .unwrap()
        .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn model_from_vfs_legacy() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.model.json",
            VfsSnapshot::file(
                r#"
                    {
                      "ClassName": "IntValue",
                      "Properties": {
                        "Value": 5
                      },
                      "Children": [
                        {
                          "Name": "The Child",
                          "ClassName": "StringValue"
                        }
                      ]
                    }
                "#,
            ),
        )
        .unwrap();

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_json_model(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo.model.json"),
            "foo",
        )
        .unwrap()
        .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}

use std::{borrow::Cow, collections::HashMap, path::Path, str, sync::Arc};

use anyhow::Context;
use memofs::Vfs;
use rbx_dom_weak::{
    types::{Attributes, Variant},
    Instance, WeakDom,
};
use serde::{Deserialize, Serialize};

use crate::{
    resolution::UnresolvedValue,
    snapshot::{
        InstanceContext, InstanceMetadata, InstanceSnapshot, MiddlewareContextAny,
        SnapshotMiddleware, ToVariantBinaryString, PRIORITY_MODEL_JSON,
    },
};

use super::util::{reconcile_meta_file_empty, try_remove_file, PathExt};

#[derive(Debug, PartialEq, Eq)]
pub struct JsonModelMiddleware;

impl SnapshotMiddleware for JsonModelMiddleware {
    fn middleware_id(&self) -> &'static str {
        "json_model"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.model.json"]
    }

    fn init_names(&self) -> &[&'static str] {
        &["init.model.json"]
    }

    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let name = if path.file_name_ends_with(".model.json") {
            path.file_name_trim_end(".model.json")?.to_owned()
        } else {
            path.file_name_trim_extension()?
        };

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
            .context(context)
            .middleware_id(self.middleware_id());

        Ok(Some(snapshot))
    }

    fn syncback_serializes_children(&self) -> bool {
        true
    }

    fn syncback_priority(
        &self,
        _dom: &rbx_dom_weak::WeakDom,
        _instance: &rbx_dom_weak::Instance,
        _consider_descendants: bool,
    ) -> Option<i32> {
        Some(PRIORITY_MODEL_JSON)
    }

    fn syncback_update(
        &self,
        vfs: &Vfs,
        path: &Path,
        diff: &crate::snapshot::DeepDiff,
        tree: &mut crate::snapshot::RojoTree,
        old_ref: rbx_dom_weak::types::Ref,
        new_dom: &rbx_dom_weak::WeakDom,
        context: &InstanceContext,
        middleware_context: Option<Arc<dyn MiddlewareContextAny>>,
    ) -> anyhow::Result<InstanceMetadata> {
        let old_inst = tree.get_instance(old_ref).unwrap();

        let new_ref = diff
            .get_matching_new_ref(old_ref)
            .with_context(|| "no matching new ref")?;
        let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;

        let my_metadata = old_inst.metadata().clone();

        let mut json_model = JsonModel::from_instance(new_dom, new_inst);
        json_model.name = None;

        let mut contents: Vec<u8> = Vec::new();
        serde_json::to_writer(&mut contents, &json_model)?;

        vfs.write(path, contents)?;

        reconcile_meta_file_empty(vfs, &path.with_extension("meta.json"))?;

        Ok(my_metadata
            .instigating_source(path)
            .context(context)
            .relevant_paths(vec![path.to_path_buf(), path.with_extension("meta.json")])
            .middleware_id(self.middleware_id()))
    }

    fn syncback_new(
        &self,
        vfs: &Vfs,
        parent_path: &Path,
        name: &str,
        new_dom: &rbx_dom_weak::WeakDom,
        new_ref: rbx_dom_weak::types::Ref,
        context: &InstanceContext,
    ) -> anyhow::Result<InstanceSnapshot> {
        let instance = new_dom.get_by_ref(new_ref).unwrap();
        let path = parent_path.join(format!("{}.model.json", name));

        let mut json_model = JsonModel::from_instance(new_dom, instance);
        json_model.name = None;

        let mut contents: Vec<u8> = Vec::new();
        serde_json::to_writer(&mut contents, &json_model)?;

        vfs.write(&path, contents)?;

        reconcile_meta_file_empty(vfs, &path.with_extension("meta.json"))?;

        Ok(
            InstanceSnapshot::from_tree_copy(new_dom, new_ref, false).metadata(
                InstanceMetadata::new()
                    .context(context)
                    .instigating_source(path.clone())
                    .relevant_paths(vec![path.clone(), path.with_extension("meta.json")])
                    .middleware_id(self.middleware_id()),
            ),
        )
    }

    fn syncback_destroy(
        &self,
        vfs: &Vfs,
        path: &Path,
        _tree: &mut crate::snapshot::RojoTree,
        _old_ref: rbx_dom_weak::types::Ref,
    ) -> anyhow::Result<()> {
        vfs.remove_file(path)?;
        try_remove_file(vfs, &path.with_extension("meta.json"))?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonModel {
    #[serde(alias = "Name", skip_serializing_if = "Option::is_none")]
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
            snapshot_id: None,
            metadata: Default::default(),
            name: Cow::Owned(name),
            class_name: Cow::Owned(class_name),
            properties,
            children,
        })
    }

    fn from_instance(dom: &WeakDom, instance: &Instance) -> Self {
        Self {
            name: Some(instance.name.clone()),
            class_name: instance.class.clone(),
            children: instance
                .children()
                .iter()
                .map(|c| JsonModel::from_instance(dom, dom.get_by_ref(*c).unwrap()))
                .collect(),
            properties: instance
                .properties
                .iter()
                .filter_map(|(k, v)| match k.as_str() {
                    "Attributes" => None,
                    _ => {
                        if Some(v)
                            != rbx_reflection_database::get()
                                .classes
                                .get(instance.class.as_str())
                                .map(|c| c.default_properties.get(k.as_str()))
                                .flatten()
                        {
                            let mut v = v.clone();

                            if let Variant::SharedString(_) = v {
                                log::trace!(
                                    "Converting {}.{} from SharedString to BinaryString",
                                    instance.class,
                                    k
                                );
                                v = v.to_variant_binary_string().unwrap();
                            }

                            Some((
                                k.clone(),
                                UnresolvedValue::from_variant_property(&instance.class, k, v),
                            ))
                        } else {
                            None
                        }
                    }
                })
                .collect(),
            attributes: instance.properties.get("attributes").map_or_else(
                || HashMap::new(),
                |attributes| match attributes {
                    Variant::Attributes(attributes) => attributes
                        .iter()
                        .map(|(k, v)| (k.clone(), UnresolvedValue::from_variant(v.clone())))
                        .collect(),
                    _ => HashMap::new(),
                },
            ),
        }
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

        let instance_snapshot = JsonModelMiddleware
            .snapshot(
                &InstanceContext::default(),
                &vfs,
                Path::new("/foo.model.json"),
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

        let instance_snapshot = JsonModelMiddleware
            .snapshot(
                &InstanceContext::default(),
                &vfs,
                Path::new("/foo.model.json"),
            )
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    str,
    sync::OnceLock,
};

use anyhow::Context;
use indexmap::IndexMap;
use memofs::Vfs;
use rbx_dom_weak::{
    types::{Attributes, Ref, Variant},
    Instance, WeakDom,
};
use serde::{Deserialize, Serialize};

use crate::{
    resolution::UnresolvedValue,
    snapshot::{
        FsSnapshot, InstanceContext, InstanceSnapshot, OptOldTuple, PropertiesFiltered,
        PropertyFilter, SnapshotMiddleware, SyncbackArgs, SyncbackNode, ToVariantBinaryString,
        PRIORITY_ALWAYS, PRIORITY_MANY_READABLE_PREFERRED, PRIORITY_MODEL_JSON,
    },
};

use super::util::PathExt;

pub fn preferred_classes() -> &'static BTreeSet<&'static str> {
    static VALUE: OnceLock<BTreeSet<&'static str>> = OnceLock::new();
    VALUE.get_or_init(|| {
        BTreeSet::from([
            "Sound",
            "SoundGroup",
            "Sky",
            "Atmosphere",
            "BloomEffect",
            "BlurEffect",
            "ColorCorrectionEffect",
            "DepthOfFieldEffect",
            "SunRaysEffect",
        ])
    })
}

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
            .middleware_id(self.middleware_id())
            .fs_snapshot(FsSnapshot::new().with_files(&[path]));

        Ok(Some(snapshot))
    }

    fn syncback_serializes_children(&self) -> bool {
        true
    }

    fn syncback_priority(
        &self,
        dom: &rbx_dom_weak::WeakDom,
        instance: &rbx_dom_weak::Instance,
        _consider_descendants: bool,
    ) -> Option<i32> {
        if instance.class == "Configuration" {
            let any_values = instance.children().iter().any(|&child_ref| {
                if let Some(child) = dom.get_by_ref(child_ref) {
                    child.class != "Folder" && child.class != "Configuration"
                } else {
                    false
                }
            });

            if any_values {
                return Some(PRIORITY_MANY_READABLE_PREFERRED);
            }
        } else if preferred_classes().contains(instance.class.as_str()) {
            return Some(PRIORITY_MANY_READABLE_PREFERRED);
        }

        Some(PRIORITY_MODEL_JSON)
    }

    fn syncback_new_path(
        &self,
        parent_path: &Path,
        name: &str,
        _instance: &Instance,
    ) -> anyhow::Result<std::path::PathBuf> {
        Ok(parent_path.join(format!("{}.model.json", name)))
    }

    fn syncback(&self, sync: &SyncbackArgs<'_, '_>) -> anyhow::Result<SyncbackNode> {
        let path = sync.path;
        let old = &sync.old;
        let new = sync.new;
        let metadata = sync.metadata;

        let (new_dom, new_ref) = new;

        let instance = new_dom.get_by_ref(new_ref).unwrap();

        let mut json_model = JsonModel::from_instance(
            new_dom,
            instance,
            &metadata.context.syncback.property_filters_save,
            &(|referent| sync.diff.is_ref_used_in_property(referent)),
        );
        json_model.name = None;

        let mut contents: Vec<u8> = Vec::new();
        serde_json::to_writer_pretty(&mut contents, &json_model)?;

        Ok(SyncbackNode::new(
            (old.opt_id(), new_ref),
            path,
            InstanceSnapshot::from_tree_copy(new_dom, new_ref, false).metadata(
                metadata
                    .clone()
                    .instigating_source(path.to_path_buf())
                    .relevant_paths(vec![path.to_path_buf(), path.with_extension("meta.json")])
                    .middleware_id(self.middleware_id())
                    .fs_snapshot(FsSnapshot::new().with_file_contents_owned(path, contents)),
            ),
        )
        .use_snapshot_children())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonModel {
    #[serde(alias = "Name", skip_serializing_if = "Option::is_none")]
    name: Option<String>,

    #[serde(alias = "ClassName")]
    class_name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    referent: Option<Ref>,

    #[serde(
        alias = "Children",
        default = "Vec::new",
        skip_serializing_if = "Vec::is_empty"
    )]
    children: Vec<JsonModel>,

    #[serde(
        alias = "Properties",
        default = "IndexMap::new",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    properties: IndexMap<String, UnresolvedValue>,

    #[serde(default = "IndexMap::new", skip_serializing_if = "IndexMap::is_empty")]
    attributes: IndexMap<String, UnresolvedValue>,
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
            preferred_ref: self.referent,
            children,
        })
    }

    fn from_instance(
        dom: &WeakDom,
        instance: &Instance,
        filters: &BTreeMap<String, PropertyFilter>,
        include_ref: &impl Fn(Ref) -> bool,
    ) -> Self {
        Self {
            name: Some(instance.name.clone()),
            class_name: instance.class.clone(),
            referent: if include_ref(instance.referent()) {
                Some(instance.referent().clone())
            } else {
                None
            },
            children: instance
                .children()
                .iter()
                .map(|c| {
                    JsonModel::from_instance(dom, dom.get_by_ref(*c).unwrap(), filters, include_ref)
                })
                .collect(),
            properties: instance
                .properties_filtered(filters, true)
                .filter_map(|(k, v)| match k {
                    "Attributes" => None,
                    _ => {
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
                            k.to_string(),
                            UnresolvedValue::from_variant_property(&instance.class, k, v),
                        ))
                    }
                })
                .collect(),
            attributes: instance.properties.get("attributes").map_or_else(
                || IndexMap::new(),
                |attributes| match attributes {
                    Variant::Attributes(attributes) => attributes
                        .iter()
                        .map(|(k, v)| (k.clone(), UnresolvedValue::from_variant(v.clone())))
                        .collect(),
                    _ => IndexMap::new(),
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

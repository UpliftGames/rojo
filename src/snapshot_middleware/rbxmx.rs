use std::{path::Path, sync::Arc};

use anyhow::Context;
use memofs::Vfs;
use rbx_dom_weak::{types::Ref, Instance, WeakDom};
use rbx_xml::EncodeOptions;

use crate::snapshot::{
    FsSnapshot, InstanceContext, InstanceMetadata, InstanceSnapshot, MiddlewareContextAny,
    SnapshotMiddleware, SnapshotOverride, PRIORITY_MODEL_XML,
};

use super::util::{reconcile_meta_file_empty, try_remove_file, PathExt};

#[derive(Debug, PartialEq, Eq)]
pub struct RbxmxMiddleware;

impl SnapshotMiddleware for RbxmxMiddleware {
    fn middleware_id(&self) -> &'static str {
        "rbxmx"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.rbxmx"]
    }

    fn init_names(&self) -> &[&'static str] {
        &["init.rbxmx"]
    }

    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let name = path.file_name_trim_extension()?;

        let options = rbx_xml::DecodeOptions::new()
            .property_behavior(rbx_xml::DecodePropertyBehavior::ReadUnknown);

        let temp_tree = rbx_xml::from_reader(vfs.read(path)?.as_slice(), options)
            .with_context(|| format!("Malformed rbxm file: {}", path.display()))?;

        let root_instance = temp_tree.root();
        let children = root_instance.children();

        if children.len() == 1 {
            let child = children[0];
            let snapshot = InstanceSnapshot::from_tree(temp_tree, child)
                .name(name)
                .metadata(
                    InstanceMetadata::new()
                        .instigating_source(path)
                        .relevant_paths(vec![path.to_path_buf()])
                        .context(context)
                        .fs_snapshot(FsSnapshot::new().with_file(path)),
                );

            Ok(Some(snapshot))
        } else {
            anyhow::bail!(
                "Rojo currently only supports model files with one top-level instance.\n\n \
                 Check the model file at path {}",
                path.display()
            );
        }
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
        Some(PRIORITY_MODEL_XML)
    }

    fn syncback_new_path(
        &self,
        parent_path: &Path,
        name: &str,
        _instance: &Instance,
    ) -> anyhow::Result<std::path::PathBuf> {
        Ok(parent_path.join(format!("{}.rbxmx", name)))
    }

    fn syncback_new(
        &self,
        vfs: &Vfs,
        path: &Path,
        new_dom: &WeakDom,
        new_ref: Ref,
        context: &InstanceContext,
        my_metadata: &InstanceMetadata,
        _overrides: Option<SnapshotOverride>,
    ) -> anyhow::Result<InstanceSnapshot> {
        let mut contents: Vec<u8> = Vec::new();
        rbx_xml::to_writer(
            &mut contents,
            new_dom,
            &[new_ref],
            EncodeOptions::new().property_behavior(rbx_xml::EncodePropertyBehavior::WriteUnknown),
        )?;

        Ok(
            InstanceSnapshot::from_tree_copy(new_dom, new_ref, false).metadata(
                InstanceMetadata::new()
                    .context(context)
                    .instigating_source(path.to_path_buf())
                    .relevant_paths(vec![path.to_path_buf(), path.with_extension("meta.json")])
                    .middleware_id(self.middleware_id())
                    .fs_snapshot(FsSnapshot::new().with_file_contents_owned(path, contents)),
            ),
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use memofs::{InMemoryFs, VfsSnapshot};

    #[test]
    fn plain_folder() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.rbxmx",
            VfsSnapshot::file(
                r#"
                    <roblox version="4">
                        <Item class="Folder" referent="0">
                            <Properties>
                                <string name="Name">THIS NAME IS IGNORED</string>
                            </Properties>
                        </Item>
                    </roblox>
                "#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = RbxmxMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.rbxmx"),
            )
            .unwrap()
            .unwrap();

        assert_eq!(instance_snapshot.name, "foo");
        assert_eq!(instance_snapshot.class_name, "Folder");
        assert_eq!(instance_snapshot.properties, Default::default());
        assert_eq!(instance_snapshot.children, Vec::new());
    }
}

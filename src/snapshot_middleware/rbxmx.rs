use std::path::Path;

use anyhow::Context;
use memofs::Vfs;
use rbx_dom_weak::{Instance, InstanceBuilder, WeakDom};
use rbx_xml::EncodeOptions;

use crate::snapshot::{
    FsSnapshot, InstanceContext, InstanceMetadata, InstanceSnapshot, OptOldTuple,
    SnapshotMiddleware, SyncbackContextX, SyncbackNode, WeakDomExtra, PRIORITY_MODEL_XML,
};

use super::util::PathExt;

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

        let mut temp_tree = rbx_xml::from_reader(vfs.read(path)?.as_slice(), options)
            .with_context(|| format!("Malformed rbxm file: {}", path.display()))?;
        temp_tree.apply_marked_external_refs(temp_tree.root_ref());

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

    fn syncback(&self, sync: &SyncbackContextX<'_, '_>) -> anyhow::Result<SyncbackNode> {
        let path = sync.path;
        let old = &sync.old;
        let new = sync.new;

        let (new_dom, new_ref) = new;

        // FIXME: This is a hack to allow mutation before save. It's not ideal
        // because we have to clone the whole save target in memory. We only
        // need to do this to add external ref attributes.
        let mut temp_tree = WeakDom::new(InstanceBuilder::from_instance(new_dom, new_ref));
        temp_tree.mark_external_refs(new_ref, &sync.diff.property_refs);

        let mut contents: Vec<u8> = Vec::new();
        rbx_xml::to_writer(
            &mut contents,
            &temp_tree,
            &[new_ref],
            EncodeOptions::new().property_behavior(rbx_xml::EncodePropertyBehavior::WriteUnknown),
        )?;

        Ok(SyncbackNode::new(
            (old.opt_id(), new_ref),
            path,
            InstanceSnapshot::from_tree_copy(new_dom, new_ref, true).metadata(
                InstanceMetadata::new()
                    .instigating_source(path.to_path_buf())
                    .relevant_paths(vec![path.to_path_buf(), path.with_extension("meta.json")])
                    .middleware_id(self.middleware_id())
                    .fs_snapshot(FsSnapshot::new().with_file_contents_owned(path, contents)),
            ),
        )
        .use_snapshot_children())
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

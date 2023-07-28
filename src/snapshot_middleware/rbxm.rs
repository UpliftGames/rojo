use std::path::Path;

use anyhow::Context;
use memofs::Vfs;
use rbx_dom_weak::Instance;

use crate::snapshot::{
    FsSnapshot, InstanceContext, InstanceMetadata, InstanceSnapshot, OptOldTuple,
    SnapshotMiddleware, SyncbackContextX, SyncbackNode, PRIORITY_MODEL_BINARY,
};

use super::util::PathExt;

#[derive(Debug, PartialEq, Eq)]
pub struct RbxmMiddleware;

impl SnapshotMiddleware for RbxmMiddleware {
    fn middleware_id(&self) -> &'static str {
        "rbxm"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.rbxm"]
    }

    fn init_names(&self) -> &[&'static str] {
        &["init.rbxm"]
    }

    #[profiling::function]
    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let name = path.file_name_trim_extension()?;

        let temp_tree = rbx_binary::from_reader(vfs.read(path)?.as_slice())
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
                        .middleware_id(self.middleware_id())
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
        Some(PRIORITY_MODEL_BINARY)
    }

    fn syncback_new_path(
        &self,
        parent_path: &Path,
        name: &str,
        _instance: &Instance,
    ) -> anyhow::Result<std::path::PathBuf> {
        Ok(parent_path.join(format!("{}.rbxm", name)))
    }

    fn syncback(&self, sync: &SyncbackContextX<'_, '_>) -> anyhow::Result<SyncbackNode> {
        let path = sync.path;
        let old = &sync.old;
        let new = sync.new;
        let metadata = sync.metadata;

        let (new_dom, new_ref) = new;

        let mut contents: Vec<u8> = Vec::new();
        rbx_binary::to_writer(&mut contents, new_dom, &[new_ref])?;

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

#[cfg(test)]
mod test {
    use super::*;

    use memofs::{InMemoryFs, VfsSnapshot};

    #[test]
    fn model_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.rbxm",
            VfsSnapshot::file(include_bytes!("../../assets/test-folder.rbxm").to_vec()),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = RbxmMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.rbxm"),
            )
            .unwrap()
            .unwrap();

        assert_eq!(instance_snapshot.name, "foo");
        assert_eq!(instance_snapshot.class_name, "Folder");
        assert_eq!(instance_snapshot.children, Vec::new());

        // We intentionally don't assert on properties. rbx_binary does not
        // distinguish between String and BinaryString. The sample model was
        // created by Roblox Studio and has an empty BinaryString "Tags"
        // property that currently deserializes incorrectly.
        // See: https://github.com/Roblox/rbx-dom/issues/49
    }
}

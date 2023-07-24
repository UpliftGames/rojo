use std::path::Path;

use anyhow::Context;
use memofs::Vfs;

use crate::snapshot::{
    InstanceContext, InstanceMetadata, InstanceSnapshot, SnapshotMiddleware, PRIORITY_MODEL_BINARY,
};

use super::util::{reconcile_meta_file_empty, try_remove_file, PathExt};

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
                        .snapshot_middleware(self.middleware_id()),
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

    fn syncback_update(
        &self,
        vfs: &Vfs,
        path: &Path,
        diff: &crate::snapshot::DeepDiff,
        tree: &mut crate::snapshot::RojoTree,
        old_ref: rbx_dom_weak::types::Ref,
        new_dom: &rbx_dom_weak::WeakDom,
        context: &InstanceContext,
    ) -> anyhow::Result<InstanceMetadata> {
        let old_inst = tree.get_instance(old_ref).unwrap();

        let new_ref = diff
            .get_matching_new_ref(old_ref)
            .with_context(|| "no matching new ref")?;
        let _new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;

        let my_metadata = old_inst.metadata().clone();

        let mut contents: Vec<u8> = Vec::new();
        rbx_binary::to_writer(&mut contents, new_dom, &[new_ref])?;

        vfs.write(path, contents)?;

        reconcile_meta_file_empty(vfs, &path.with_extension("meta.json"))?;

        Ok(my_metadata
            .instigating_source(path.clone())
            .context(context)
            .relevant_paths(vec![path.to_path_buf(), path.with_extension("meta.json")])
            .snapshot_middleware(self.middleware_id()))
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
        let _instance = new_dom.get_by_ref(new_ref).unwrap();
        let path = parent_path.join(format!("{}.rbxm", name));

        let mut contents: Vec<u8> = Vec::new();
        rbx_binary::to_writer(&mut contents, new_dom, &[new_ref])?;

        vfs.write(&path, contents)?;

        reconcile_meta_file_empty(vfs, &path.with_extension("meta.json"))?;

        Ok(
            InstanceSnapshot::from_tree_copy(new_dom, new_ref, false).metadata(
                InstanceMetadata::new()
                    .context(context)
                    .instigating_source(path.clone())
                    .relevant_paths(vec![path.clone(), path.with_extension("meta.json")])
                    .snapshot_middleware(self.middleware_id()),
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

use std::{collections::HashSet, path::Path, str, sync::Arc};

use anyhow::{bail, Context};
use maplit::hashmap;
use memofs::{IoResultExt, Vfs};
use rbx_dom_weak::{
    types::{Ref, Variant},
    Instance, WeakDom,
};

use crate::snapshot::{
    DeepDiff, InstanceContext, InstanceMetadata, InstanceSnapshot, MiddlewareContextAny, RojoTree,
    SnapshotMiddleware, PRIORITY_SINGLE_READABLE,
};

use super::{
    meta_file::MetadataFile,
    util::{reconcile_meta_file, try_remove_file, PathExt},
};

#[derive(Debug, PartialEq, Eq)]
pub struct TxtMiddleware;

impl SnapshotMiddleware for TxtMiddleware {
    fn middleware_id(&self) -> &'static str {
        "txt"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.txt"]
    }

    fn init_names(&self) -> &[&'static str] {
        &["init.txt"]
    }

    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let name = path.file_name_trim_extension()?;

        let contents = vfs.read(path)?;
        let contents_str = str::from_utf8(&contents)
            .with_context(|| format!("File was not valid UTF-8: {}", path.display()))?
            .to_owned();

        let properties = hashmap! {
            "Value".to_owned() => contents_str.into(),
        };

        let meta_path = path.with_file_name(format!("{}.meta.json", name));

        let mut snapshot = InstanceSnapshot::new()
            .name(name)
            .class_name("StringValue")
            .properties(properties)
            .metadata(
                InstanceMetadata::new()
                    .instigating_source(path)
                    .relevant_paths(vec![path.to_path_buf(), meta_path.clone()])
                    .context(context)
                    .snapshot_middleware(self.middleware_id()),
            );

        if let Some(meta_contents) = vfs.read(&meta_path).with_not_found()? {
            let mut metadata = MetadataFile::from_slice(&meta_contents, meta_path)?;
            metadata.apply_all(&mut snapshot)?;
        }

        Ok(Some(snapshot))
    }

    fn syncback_priority(
        &self,
        _dom: &WeakDom,
        instance: &rbx_dom_weak::Instance,
        consider_descendants: bool,
    ) -> Option<i32> {
        if consider_descendants && !instance.children().is_empty() {
            return None;
        }

        if instance.class == "StringValue" {
            Some(PRIORITY_SINGLE_READABLE)
        } else {
            None
        }
    }

    fn syncback_update(
        &self,
        vfs: &Vfs,
        path: &Path,
        diff: &DeepDiff,
        tree: &mut RojoTree,
        old_ref: Ref,
        new_dom: &WeakDom,
        context: &InstanceContext,
        middleware_context: Option<Arc<dyn MiddlewareContextAny>>,
    ) -> anyhow::Result<InstanceMetadata> {
        let old_inst = tree.get_instance(old_ref).unwrap();

        let new_ref = diff
            .get_matching_new_ref(old_ref)
            .with_context(|| "no matching new ref")?;
        let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;

        let my_metadata = old_inst.metadata().clone();

        vfs.write(path, get_instance_contents(new_inst)?)?;

        reconcile_meta_file(
            vfs,
            &path.with_extension("meta.json"),
            new_inst,
            HashSet::from(["Value", "ClassName"]),
            Some("StringValue"),
        )?;

        Ok(my_metadata
            .instigating_source(path)
            .context(context)
            .relevant_paths(vec![path.to_path_buf(), path.with_extension("meta.json")])
            .snapshot_middleware(self.middleware_id()))
    }

    fn syncback_new(
        &self,
        vfs: &Vfs,
        parent_path: &Path,
        name: &str,
        new_dom: &WeakDom,
        new_ref: Ref,
        context: &InstanceContext,
    ) -> anyhow::Result<InstanceSnapshot> {
        let instance = new_dom.get_by_ref(new_ref).unwrap();
        let path = parent_path.join(format!("{}.txt", name));

        vfs.write(&path, get_instance_contents(instance)?)?;

        reconcile_meta_file(
            vfs,
            &path.with_extension("meta.json"),
            instance,
            HashSet::from(["Value", "ClassName"]),
            Some("StringValue"),
        )?;

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
        _tree: &mut RojoTree,
        _old_ref: Ref,
    ) -> anyhow::Result<()> {
        vfs.remove_file(path)?;
        try_remove_file(vfs, &path.with_extension("meta.json"))?;
        Ok(())
    }
}

fn get_instance_contents(instance: &Instance) -> anyhow::Result<&str> {
    Ok(match instance.properties.get("Value") {
        Some(Variant::String(contents)) => contents.as_str(),
        Some(Variant::BinaryString(contents)) => str::from_utf8(&contents.as_ref())?,
        Some(Variant::SharedString(contents)) => str::from_utf8(&contents.data())?,
        _ => bail!("StringValue.Value was not a string or was missing"),
    })
}

#[cfg(test)]
mod test {
    use super::*;

    use memofs::{InMemoryFs, VfsSnapshot};

    #[test]
    fn instance_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot("/foo.txt", VfsSnapshot::file("Hello there!"))
            .unwrap();

        let mut vfs = Vfs::new(imfs.clone());

        let instance_snapshot = TxtMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/foo.txt"))
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}

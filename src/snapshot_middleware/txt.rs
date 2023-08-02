use std::{collections::HashSet, path::Path, str};

use anyhow::{bail, Context};
use maplit::hashmap;
use memofs::{IoResultExt, Vfs};
use rbx_dom_weak::{types::Variant, Instance, WeakDom};

use crate::snapshot::{
    FsSnapshot, InstanceContext, InstanceMetadata, InstanceSnapshot, OptOldTuple,
    SnapshotMiddleware, SyncbackContextX, SyncbackNode, PRIORITY_SINGLE_READABLE,
};

use super::{
    meta_file::MetadataFile,
    util::{reconcile_meta_file, PathExt},
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
                    .middleware_id(self.middleware_id())
                    .fs_snapshot(FsSnapshot::new().with_files(&[path, &meta_path])),
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

    fn syncback_new_path(
        &self,
        parent_path: &Path,
        name: &str,
        _instance: &Instance,
    ) -> anyhow::Result<std::path::PathBuf> {
        Ok(parent_path.join(format!("{}.txt", name)))
    }

    fn syncback(&self, sync: &SyncbackContextX<'_, '_>) -> anyhow::Result<SyncbackNode> {
        let vfs = sync.vfs;
        let path = sync.path;
        let old = &sync.old;
        let new = sync.new;
        let metadata = sync.metadata;

        let (new_dom, new_ref) = new;

        let instance = new_dom.get_by_ref(new_ref).unwrap();

        let meta = reconcile_meta_file(
            vfs,
            &path.with_extension("meta.json"),
            instance,
            sync.ref_for_save_if_used(),
            HashSet::from(["Value", "ClassName"]),
            Some("StringValue"),
            &metadata.context.syncback.property_filters_save,
        )?;

        Ok(SyncbackNode::new(
            (old.opt_id(), new_ref),
            path,
            InstanceSnapshot::from_tree_copy(new_dom, new_ref, false)
                .metadata(
                    metadata
                        .clone()
                        .instigating_source(path.to_path_buf())
                        .relevant_paths(vec![path.to_path_buf(), path.with_extension("meta.json")])
                        .middleware_id(self.middleware_id())
                        .fs_snapshot(
                            FsSnapshot::new()
                                .with_file_contents_borrowed(
                                    &path,
                                    get_instance_contents(instance)?,
                                )
                                .with_file_contents_opt(&path.with_extension("meta.json"), meta),
                        ),
                )
                .preferred_ref(sync.ref_for_save()),
        ))
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

use std::path::Path;

use anyhow::Context;
use memofs::Vfs;
use rbx_dom_weak::WeakDom;

use crate::{
    snapshot::{apply_patch_set, compute_patch_set, InstanceContext, InstanceSnapshot, RojoTree},
    snapshot_middleware::{snapshot_from_vfs, PathExt},
};

#[derive(Debug)]
pub enum InputTree {
    RojoTree(RojoTree),
    WeakDom(WeakDom),
}

impl Into<WeakDom> for InputTree {
    fn into(self) -> WeakDom {
        match self {
            InputTree::RojoTree(tree) => tree.into_weakdom(),
            InputTree::WeakDom(tree) => tree,
        }
    }
}

impl AsRef<WeakDom> for InputTree {
    fn as_ref(&self) -> &WeakDom {
        match self {
            InputTree::RojoTree(tree) => tree.inner(),
            InputTree::WeakDom(tree) => tree,
        }
    }
}

pub fn open_tree_at_location(vfs: &Vfs, path: &Path) -> Result<InputTree, anyhow::Error> {
    if path.file_name_ends_with(".rbxl") || path.file_name_ends_with(".rbxm") {
        return rbx_binary::from_reader(vfs.read(path)?.as_slice())
            .with_context(|| format!("Malformed rbx binary file: {}", path.display()))
            .map(InputTree::WeakDom);
    } else if path.file_name_ends_with(".rbxlx") || path.file_name_ends_with(".rbxmx") {
        let options = rbx_xml::DecodeOptions::new()
            .property_behavior(rbx_xml::DecodePropertyBehavior::ReadUnknown);

        return rbx_xml::from_reader(vfs.read(path)?.as_slice(), options)
            .with_context(|| format!("Malformed rbx xml file: {}", path.display()))
            .map(InputTree::WeakDom);
    }

    let mut tree = RojoTree::new(InstanceSnapshot::new());

    let root_id = tree.get_root_id();

    let instance_context = InstanceContext::default();

    log::trace!("Generating snapshot of instances from VFS");
    let snapshot = snapshot_from_vfs(&instance_context, vfs, path)?;

    log::trace!("Computing initial patch set");
    let patch_set = compute_patch_set(snapshot, &tree, root_id);

    log::trace!("Applying initial patch set");
    apply_patch_set(&mut tree, patch_set);

    log::trace!("Opened tree; returning dom");

    Ok(InputTree::RojoTree(tree))
}

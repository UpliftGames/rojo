use std::{
    borrow::BorrowMut,
    collections::BTreeMap,
    mem::forget,
    path::{Path, PathBuf},
};

use crate::{
    serve_session::ServeSession,
    snapshot::{
        apply_patch_set, compute_patch_set, DeepDiff, InstanceContext, InstanceSnapshot, RojoTree,
    },
    snapshot_middleware::{snapshot_from_vfs, PathExt},
};
use anyhow::{bail, Context};
use clap::Parser;
use fs_err::File;
use memofs::Vfs;
use rbx_dom_weak::WeakDom;

/// Displays a diff between two inputs.
#[derive(Debug, Parser)]
pub struct DiffCommand {
    /// Path to the "old" diff input. Can be a project file, rbxm(x), rbxl(x).
    pub old: PathBuf,
    /// Path to the "new" diff input. Can be a project file, rbxm(x), rbxl(x).
    pub new: PathBuf,

    /// Path to the object to diff in the tree.
    pub path: Option<String>,
}

fn get_tree_at_location(vfs: &Vfs, path: &Path) -> Result<WeakDom, anyhow::Error> {
    if path.file_name_ends_with(".rbxl") || path.file_name_ends_with(".rbxm") {
        return Ok(rbx_binary::from_reader(vfs.read(path)?.as_slice())
            .with_context(|| format!("Malformed rbx binary file: {}", path.display()))?);
    } else if path.file_name_ends_with(".rbxlx") || path.file_name_ends_with(".rbxmx") {
        let options = rbx_xml::DecodeOptions::new()
            .property_behavior(rbx_xml::DecodePropertyBehavior::ReadUnknown);

        return Ok(rbx_xml::from_reader(vfs.read(path)?.as_slice(), options)
            .with_context(|| format!("Malformed rbx xml file: {}", path.display()))?);
    }

    let mut tree = RojoTree::new(InstanceSnapshot::new());

    let root_id = tree.get_root_id();

    let instance_context = InstanceContext::default();

    log::trace!("Generating snapshot of instances from VFS");
    let snapshot = snapshot_from_vfs(&instance_context, &vfs, &path)?;

    log::trace!("Computing initial patch set");
    let patch_set = compute_patch_set(snapshot, &tree, root_id);

    log::trace!("Applying initial patch set");
    apply_patch_set(&mut tree, patch_set);

    log::trace!("Opened tree; returning dom");

    Ok(tree.into_weakdom())
}

impl DiffCommand {
    pub fn run(self) -> anyhow::Result<()> {
        let vfs = Vfs::new_default();

        let old_tree = get_tree_at_location(&vfs, &self.old)?;
        let mut new_tree = get_tree_at_location(&vfs, &self.new)?;
        let new_root_ref = new_tree.root_ref();

        log::trace!("Opened both trees; about to create diff");

        let empty_filters = BTreeMap::new();
        let diff = DeepDiff::new(
            &old_tree,
            old_tree.root_ref(),
            &mut new_tree,
            new_root_ref,
            |_| &empty_filters,
        );

        let path_parts: Option<Vec<String>> = self
            .path
            .map(|v| v.split(".").map(str::to_string).collect());

        log::trace!("Created diff; about to show diff");

        diff.show_diff(&old_tree, &new_tree, &path_parts.unwrap_or(vec![]));

        // Leak objects that would cause a delay while running destructors.
        // We're about to close, and the destructors do nothing important.
        forget(old_tree);
        forget(new_tree);
        forget(diff);

        Ok(())
    }
}

#[test]
fn test_diff() -> Result<(), anyhow::Error> {
    let log_env = env_logger::Env::default();

    env_logger::Builder::from_env(log_env)
        .format_module_path(false)
        .format_timestamp(None)
        // Indent following lines equal to the log level label, like `[ERROR] `
        .format_indent(Some(8))
        .init();

    let old_path = PathBuf::from("C:/Projects/Uplift/adopt-me/default.project.json");
    let new_path = PathBuf::from("C:/Projects/Uplift/adopt-me/game.rbxl");
    // let old_path = PathBuf::from("C:/Projects/Uplift/rojo/syncback_test/default.project.json");
    // let new_path = PathBuf::from("C:/Projects/Uplift/rojo/syncback_test/game.rbxl");

    DiffCommand {
        old: old_path,
        new: new_path,
        path: Some("Workspace".to_string()),
    }
    .run()?;

    Ok(())
}

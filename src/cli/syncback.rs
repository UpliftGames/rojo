use std::{
    borrow::BorrowMut,
    io::Write,
    mem::forget,
    path::{Path, PathBuf},
};

use crate::{
    snapshot::{apply_patch_set, compute_patch_set, InstanceContext, InstanceSnapshot, RojoTree},
    snapshot_middleware::snapshot_from_vfs,
};
use anyhow::{bail, Context};
use clap::Parser;
use fs_err::File;
use memofs::Vfs;

use super::resolve_path;

const UNKNOWN_INPUT_KIND_ERR: &str = "Could not detect what kind of file to sync from. \
                                       Expected output file to end in .rbxl, .rbxlx, .rbxm, or .rbxmx.";

fn get_tree_at_location(vfs: &Vfs, path: &Path) -> Result<RojoTree, anyhow::Error> {
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

    Ok(tree)
}

/// Syncs changes back to the filesystem from a model or place file.
#[derive(Debug, Parser)]
pub struct SyncbackCommand {
    /// Path to the project to serve. Defaults to the current directory.
    #[clap(default_value = "")]
    pub project: PathBuf,

    /// The file to sync back from.
    ///
    /// Should end in .rbxm, .rbxl, .rbxmx, or .rbxlx.
    #[clap(long, short)]
    pub input: PathBuf,

    /// Skip (say "yes" to) the diff viewer confirmation screen.
    #[clap(short = 'y', long, required = false)]
    pub non_interactive: bool,
}

impl SyncbackCommand {
    pub fn run(self) -> anyhow::Result<()> {
        let project_path = resolve_path(&self.project);

        let output_kind = detect_input_kind(&self.input).context(UNKNOWN_INPUT_KIND_ERR)?;

        log::trace!("Constructing in-memory filesystem");

        let vfs = Vfs::new_default();
        let mut tree = get_tree_at_location(&vfs, &project_path)?;

        let result = syncback(
            &vfs,
            &mut tree,
            &self.input,
            output_kind,
            self.non_interactive,
        );

        log::trace!("syncback out");
        if let Err(e) = result {
            log::trace!("{:#?}", e);
            bail!(e);
        }

        // Avoid dropping tree: it's potentially VERY expensive to drop
        // and we're about to exit anyways.
        forget(tree);

        Ok(())
    }
}

/// The different kinds of output that Rojo can build to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    /// An XML model file.
    Rbxmx,

    /// An XML place file.
    Rbxlx,

    /// A binary model file.
    Rbxm,

    /// A binary place file.
    Rbxl,
}

fn detect_input_kind(input: &Path) -> Option<InputKind> {
    let extension = input.extension()?.to_str()?;

    match extension {
        "rbxlx" => Some(InputKind::Rbxlx),
        "rbxmx" => Some(InputKind::Rbxmx),
        "rbxl" => Some(InputKind::Rbxl),
        "rbxm" => Some(InputKind::Rbxm),
        _ => None,
    }
}

fn xml_encode_config() -> rbx_xml::DecodeOptions {
    rbx_xml::DecodeOptions::new().property_behavior(rbx_xml::DecodePropertyBehavior::ReadUnknown)
}

#[profiling::function]
fn syncback(
    vfs: &Vfs,
    tree: &mut RojoTree,
    output: &Path,
    output_kind: InputKind,
    skip_prompt: bool,
) -> anyhow::Result<()> {
    let tree = tree.borrow_mut();
    let root_id = tree.get_root_id();

    // log::trace!("Tree: {:#?}", tree);

    tree.warn_for_broken_refs();

    log::trace!("Opening input file");
    let mut file = File::open(output)?;

    let mut new_dom = match output_kind {
        InputKind::Rbxmx | InputKind::Rbxlx => {
            rbx_xml::from_reader(&mut file, xml_encode_config())?
        }
        InputKind::Rbxm | InputKind::Rbxl => rbx_binary::from_reader(&mut file)?,
    };
    let new_root = new_dom.root_ref();

    log::trace!("Diffing and applying changes");

    // diff.show_diff(&old_tree, &new_tree, &path_parts.unwrap_or(vec![]));

    let diff = tree.syncback_start(vfs, root_id, &mut new_dom, new_root);

    if !skip_prompt {
        println!("The following is a diff of the changes to be synced back to the filesystem:");
        diff.show_diff(
            tree.inner(),
            &new_dom,
            &Vec::new(),
            |old_ref| tree.syncback_get_filters(old_ref),
            |old_ref| tree.syncback_should_skip(old_ref),
        );
        println!("\nDo you want to continue and apply these changes? [Y/n]");
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" && input.trim() != "" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    tree.syncback_process(vfs, &diff, root_id, &new_dom)?;

    tree.warn_for_broken_refs();

    Ok(())
}

#[test]
fn test_syncback() -> Result<(), anyhow::Error> {
    let log_env = env_logger::Env::default();

    env_logger::Builder::from_env(log_env)
        .format_module_path(false)
        .format_timestamp(None)
        // Indent following lines equal to the log level label, like `[ERROR] `
        .format_indent(Some(8))
        .init();

    let input = PathBuf::from("C:/Projects/Uplift/adopt-me/game.rbxl");
    let project_path = PathBuf::from("C:/Projects/Uplift/adopt-me/default.project.json");

    // println!("Press enter when profiler is attached");
    // std::io::stdin().read_line(&mut String::new()).ok();

    // let input = PathBuf::from("C:/Projects/Uplift/rojo/syncback_test/game.rbxl");
    // let project_path = PathBuf::from("C:/Projects/Uplift/rojo/syncback_test/default.project.json");

    let input_kind = detect_input_kind(&input).context(UNKNOWN_INPUT_KIND_ERR)?;

    log::trace!("Constructing in-memory filesystem");

    let vfs = Vfs::new_default();
    let mut tree = get_tree_at_location(&vfs, &project_path)?;

    let result = syncback(&vfs, &mut tree, &input, input_kind, false);

    log::trace!("syncback out");
    if let Err(e) = result {
        log::trace!("{:#?}", e);
        bail!(e);
    }

    // Avoid dropping tree: it's potentially VERY expensive to drop
    // and we're about to exit anyways.
    forget(tree);

    Ok(())
}

use std::{
    borrow::BorrowMut,
    io::Write,
    mem::forget,
    path::{Path, PathBuf},
};

use crate::{
    open_tree::{open_tree_at_location, InputTree},
    snapshot::{apply_patch_set, compute_patch_set, InstanceContext, InstanceSnapshot, RojoTree},
    snapshot_middleware::snapshot_from_vfs,
};
use anyhow::{bail, Context};
use clap::Parser;
use fs_err::File;
use memofs::Vfs;
use rbx_dom_weak::WeakDom;

use super::resolve_path;

const UNKNOWN_INPUT_KIND_ERR: &str = "Could not detect what kind of file to sync from. \
                                       Expected output file to end in .rbxl, .rbxlx, .rbxm, or .rbxmx.";

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

        log::trace!("Constructing in-memory filesystem");

        let vfs = Vfs::new_default();

        log::info!("Opening project tree...");
        let timer = std::time::Instant::now();

        let tree = open_tree_at_location(&vfs, &project_path)?;
        let mut tree = match tree {
            InputTree::RojoTree(tree) => tree,
            InputTree::WeakDom(_) => bail!("Syncback can only sync into Rojo projects, not raw model files. Did you mean to specify {} as --input?", project_path.display()),
        };

        log::info!(
            "  opened project tree in {:.3}s",
            timer.elapsed().as_secs_f64()
        );

        let result = syncback(&vfs, &mut tree, &self.input, self.non_interactive);

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

#[profiling::function]
fn syncback(vfs: &Vfs, tree: &mut RojoTree, input: &Path, skip_prompt: bool) -> anyhow::Result<()> {
    let tree = tree.borrow_mut();
    let root_id = tree.get_root_id();

    // log::trace!("Tree: {:#?}", tree);

    tree.warn_for_broken_refs();

    log::info!("Opening syncback input file...");
    let timer = std::time::Instant::now();

    let mut new_dom: WeakDom = open_tree_at_location(vfs, &input)?.into();
    let new_root = new_dom.root_ref();

    log::info!(
        "  opened syncback input file in {:.3}s",
        timer.elapsed().as_secs_f64()
    );

    log::info!("Diffing project and input file...");
    let timer = std::time::Instant::now();

    let diff = tree.syncback_start(vfs, root_id, &mut new_dom, new_root);

    log::info!(
        "  diffed project and input file in {:.3}s",
        timer.elapsed().as_secs_f64()
    );

    if !skip_prompt {
        println!("The following is a diff of the changes to be synced back to the filesystem:");
        let any_changes = diff.show_diff(
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

    log::info!("Applying changes...");
    let timer = std::time::Instant::now();

    tree.syncback_process(vfs, &diff, root_id, &new_dom)?;

    tree.warn_for_broken_refs();

    log::info!("  applied changes in {:.3}s", timer.elapsed().as_secs_f64());

    // Avoid dropping tree: it's potentially VERY expensive to drop
    forget(new_dom);

    Ok(())
}

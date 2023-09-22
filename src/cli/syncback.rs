use std::{
    borrow::BorrowMut,
    io::Write,
    mem::forget,
    path::{Path, PathBuf},
};

use crate::{
    open_tree::{open_tree_at_location, InputTree},
    snapshot::{DiffOptions, DiffOptionsCommand, RojoTree},
};
use anyhow::bail;
use clap::Parser;

use memofs::Vfs;
use rbx_dom_weak::WeakDom;

use super::resolve_path;

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

    #[clap(flatten)]
    pub diff_options: DiffOptionsCommand,
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

        let diff_options = DiffOptions {
            basic_comparison: true,
            deduplication_attributes: true,
            rescan_ref_fix: false,
            deep_comparison: false,
            deep_comparison_depth: 2,
        }
        .apply_command_args(self.diff_options);

        let result = syncback(
            &vfs,
            &mut tree,
            &self.input,
            diff_options,
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

#[profiling::function]
fn syncback(
    vfs: &Vfs,
    tree: &mut RojoTree,
    input: &Path,
    diff_options: DiffOptions,
    skip_prompt: bool,
) -> anyhow::Result<()> {
    let tree = tree.borrow_mut();
    let root_id = tree.get_root_id();

    // log::trace!("Tree: {:#?}", tree);

    tree.warn_for_broken_refs();

    log::info!("Opening syncback input file...");
    let timer = std::time::Instant::now();

    let mut new_dom: WeakDom = open_tree_at_location(vfs, input)?.into();
    let new_root = new_dom.root_ref();

    log::info!(
        "  opened syncback input file in {:.3}s",
        timer.elapsed().as_secs_f64()
    );

    log::info!("Diffing project and input file...");
    let timer = std::time::Instant::now();

    let diff = tree.syncback_start(vfs, root_id, &mut new_dom, new_root, diff_options.clone());

    log::info!(
        "  diffed project and input file in {:.3}s",
        timer.elapsed().as_secs_f64()
    );

    if !skip_prompt {
        println!("The following is a diff of the changes to be synced back to the filesystem:");
        if diff_options.deduplication_attributes {
            println!("  Because deduplication attributes are turned on, additional changes");
            println!("  may be written to the filesystem to update deduplication attributes.");
        }

        let _any_changes = diff.show_diff(
            tree.inner(),
            &new_dom,
            &Vec::new(),
            diff_options.clone(),
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

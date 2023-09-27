use std::{mem::forget, path::PathBuf};

use crate::{
    open_tree::{open_tree_at_location, InputTree},
    snapshot::{default_filters_diff, empty_hashset, DeepDiff, DiffOptions, DiffOptionsCommand},
};

use clap::Parser;

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

    #[clap(flatten)]
    pub diff_options: DiffOptionsCommand,
}

impl DiffCommand {
    pub fn run(self) -> anyhow::Result<()> {
        let vfs = Vfs::new_default();

        log::info!("Opening old tree...");
        let timer = std::time::Instant::now();

        let old_tree = open_tree_at_location(&vfs, &self.old)?;
        let old_dom = old_tree.as_ref();

        log::info!("  opened old tree in {:.3}s", timer.elapsed().as_secs_f64());

        log::info!("Opening new tree...");
        let timer = std::time::Instant::now();

        let mut new_dom: WeakDom = open_tree_at_location(&vfs, &self.new)?.into();
        let new_root_ref = new_dom.root_ref();

        log::info!("  opened new tree in {:.3}s", timer.elapsed().as_secs_f64());

        log::info!("Diffing trees...");
        let timer = std::time::Instant::now();

        let diff_options = DiffOptions {
            basic_comparison: true,
            deduplication_attributes: false,
            rescan_ref_fix: true,
            deep_comparison: true,
            deep_comparison_depth: 2,
        }
        .apply_command_args(self.diff_options);

        let diff = DeepDiff::new(
            old_dom,
            old_dom.root_ref(),
            &mut new_dom,
            new_root_ref,
            diff_options.clone(),
            |old_ref| match &old_tree {
                InputTree::RojoTree(tree) => tree.syncback_get_filters(old_ref),
                InputTree::WeakDom(_) => default_filters_diff(),
            },
            |old_ref| match &old_tree {
                InputTree::RojoTree(tree) => tree.syncback_should_skip(old_ref),
                InputTree::WeakDom(_) => false,
            },
            |old_ref| match &old_tree {
                InputTree::RojoTree(tree) => tree.syncback_get_skip_instance_names(old_ref),
                InputTree::WeakDom(_) => empty_hashset(),
            },
        );

        let path_parts: Option<Vec<String>> = self
            .path
            .map(|v| v.split('.').map(str::to_string).collect());

        diff.show_diff(
            old_dom,
            &new_dom,
            &path_parts.unwrap_or(vec![]),
            diff_options.clone(),
            |old_ref| match &old_tree {
                InputTree::RojoTree(tree) => tree.syncback_get_filters(old_ref),
                InputTree::WeakDom(_) => default_filters_diff(),
            },
            |old_ref| match &old_tree {
                InputTree::RojoTree(tree) => tree.syncback_should_skip(old_ref),
                InputTree::WeakDom(_) => false,
            },
        );

        log::info!("diffed trees in {:.3}s", timer.elapsed().as_secs_f64());

        // Leak objects that would cause a delay while running destructors.
        // We're about to close, and the destructors do nothing important.
        forget(old_tree);
        forget(new_dom);
        forget(diff);

        Ok(())
    }
}

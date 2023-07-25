//! Defines the semantics that Rojo uses to turn entries on the filesystem into
//! Roblox instances using the instance snapshot subsystem.
//!
//! These modules define how files turn into instances.

#![allow(dead_code)]

mod csv;
mod dir;
mod json;
mod json_model;
mod lua;
mod meta_file;
mod project;
mod rbxm;
mod rbxmx;
mod toml;
mod txt;
mod util;

use std::sync::Arc;
use std::{collections::HashMap, path::Path, sync::OnceLock};

use globset::{Glob, GlobSet, GlobSetBuilder};

use memofs::{IoResultExt, Vfs};

use crate::snapshot::{InstanceContext, InstanceSnapshot, SnapshotMiddleware};

use self::{
    csv::CsvMiddleware, dir::DirectoryMiddleware, json::JsonMiddleware,
    json_model::JsonModelMiddleware, lua::LuaMiddleware, project::ProjectMiddleware,
    rbxm::RbxmMiddleware, rbxmx::RbxmxMiddleware, toml::TomlMiddleware, txt::TxtMiddleware,
};

pub use self::meta_file::MetadataFile;
pub use self::project::snapshot_project_node;

pub fn get_middlewares_ordered() -> &'static Vec<Arc<dyn SnapshotMiddleware>> {
    static MIDDLEWARES: OnceLock<Vec<Arc<dyn SnapshotMiddleware>>> = OnceLock::new();

    MIDDLEWARES.get_or_init(|| {
        vec![
            Arc::new(CsvMiddleware),
            Arc::new(TxtMiddleware),
            Arc::new(TomlMiddleware),
            Arc::new(LuaMiddleware),
            Arc::new(RbxmMiddleware),
            Arc::new(RbxmxMiddleware),
            Arc::new(DirectoryMiddleware),
            Arc::new(ProjectMiddleware),
            Arc::new(JsonModelMiddleware),
            Arc::new(JsonMiddleware),
        ]
    })
}

pub fn get_middlewares() -> &'static HashMap<&'static str, Arc<dyn SnapshotMiddleware>> {
    static MIDDLEWARES: OnceLock<HashMap<&'static str, Arc<dyn SnapshotMiddleware>>> =
        OnceLock::new();

    MIDDLEWARES.get_or_init(|| {
        get_middlewares_ordered()
            .iter()
            .map(|m| (m.middleware_id(), m.clone()))
            .collect()
    })
}

pub fn get_middleware_inits() -> &'static HashMap<&'static str, &'static str> {
    static MIDDLEWARES_INIT: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

    MIDDLEWARES_INIT.get_or_init(|| {
        get_middlewares()
            .iter()
            .flat_map(|(&provider_id, middleware)| {
                middleware
                    .init_names()
                    .iter()
                    .map(move |&init_name| (init_name, provider_id))
            })
            .collect()
    })
}

pub fn get_middleware_globs() -> &'static Vec<(&'static str, GlobSet, GlobSet)> {
    static MIDDLEWARES_GLOBS: OnceLock<Vec<(&'static str, GlobSet, GlobSet)>> = OnceLock::new();

    MIDDLEWARES_GLOBS.get_or_init(|| {
        get_middlewares_ordered()
            .iter()
            .map(|middleware| {
                let mut include_builder = GlobSetBuilder::new();
                middleware.default_globs().iter().for_each(|&glob| {
                    include_builder.add(Glob::new(glob).unwrap());
                });

                let mut exclude_builder = GlobSetBuilder::new();
                middleware.exclude_globs().iter().for_each(|&glob| {
                    exclude_builder.add(Glob::new(glob).unwrap());
                });

                (
                    middleware.middleware_id(),
                    include_builder.build().unwrap(),
                    exclude_builder.build().unwrap(),
                )
            })
            .collect()
    })
}

/// The main entrypoint to the snapshot function. This function can be pointed
/// at any path and will return something if Rojo knows how to deal with it.
#[profiling::function]
pub fn snapshot_from_vfs(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let _meta = match vfs.metadata(path).with_not_found()? {
        Some(meta) => meta,
        None => return Ok(None),
    };

    for (provider_id, include_glob, exclude_glob) in get_middleware_globs() {
        // trace!(
        //     "Checking if {:?} matches {:?}: {} {}",
        //     get_middlewares()[provider_id].default_globs(),
        //     path,
        //     include_glob.is_match(path),
        //     if exclude_glob.is_match(path) {
        //         "(exclude)"
        //     } else {
        //         "(included)"
        //     }
        // );
        if include_glob.is_match(path) && !exclude_glob.is_match(path) {
            if get_middlewares()[provider_id].match_only_directories() {
                if vfs.metadata(path)?.is_file() {
                    continue;
                }
            }

            return get_middlewares()[provider_id].snapshot(context, vfs, path);
        }
    }

    Ok(None)

    // TODO: handle transformer settings
    // TODO: make sure globs handle directories properly
}

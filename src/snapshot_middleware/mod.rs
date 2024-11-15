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

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::Context;
use memofs::{IoResultExt, Vfs};
use rbx_dom_weak::{types::Variant, Instance};
use serde::{Deserialize, Serialize};

use crate::{
    glob::Glob,
    resolution::UnresolvedValue,
    syncback::{SyncbackReturn, SyncbackSnapshot},
    variant_eq::variant_eq,
    InstanceWithMeta,
};
use crate::{
    snapshot::{InstanceContext, InstanceSnapshot, SyncRule},
    syncback::validate_file_name,
};

use self::{
    csv::{snapshot_csv, snapshot_csv_init, syncback_csv, syncback_csv_init},
    dir::{snapshot_dir, syncback_dir},
    json::snapshot_json,
    json_model::{snapshot_json_model, syncback_json_model},
    lua::{snapshot_lua, snapshot_lua_init, syncback_lua, syncback_lua_init},
    project::{snapshot_project, syncback_project},
    rbxm::{snapshot_rbxm, syncback_rbxm},
    rbxmx::{snapshot_rbxmx, syncback_rbxmx},
    toml::snapshot_toml,
    txt::{snapshot_txt, syncback_txt},
};

pub use self::{
    lua::ScriptType, project::snapshot_project_node, util::emit_legacy_scripts_default,
    util::PathExt,
};

/// Returns an `InstanceSnapshot` for the provided path.
/// This will inspect the path and find the appropriate middleware for it,
/// taking user-written rules into account. Then, it will attempt to convert
/// the path into an InstanceSnapshot using that middleware.
#[profiling::function]
pub fn snapshot_from_vfs(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let meta = match vfs.metadata(path).with_not_found()? {
        Some(meta) => meta,
        None => return Ok(None),
    };

    if meta.is_dir() {
        let (middleware, dir_name, init_path) = get_dir_middleware(vfs, path)?;
        // TODO: Support user defined init paths
        // If and when we do, make sure to go support it in
        // `Project::set_file_name`, as right now it special-cases
        // `default.project.json` as an `init` path.
        match middleware {
            Middleware::Dir => middleware.snapshot(context, vfs, path, dir_name),
            _ => middleware.snapshot(context, vfs, &init_path, dir_name),
        }
    } else {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .with_context(|| format!("file name of {} is invalid", path.display()))?;

        // TODO: Is this even necessary anymore?
        match file_name {
            "init.server.luau" | "init.server.lua" | "init.client.luau" | "init.client.lua"
            | "init.luau" | "init.lua" | "init.csv" => return Ok(None),
            _ => {}
        }

        snapshot_from_path(context, vfs, path)
    }
}

/// Gets the appropriate middleware for a directory by checking for `init`
/// files. This uses an intrinsic priority list and for compatibility,
/// that order should be left unchanged.
///
/// Returns the middleware, the name of the directory, and the path to
/// the init location.
fn get_dir_middleware<'path>(
    vfs: &Vfs,
    dir_path: &'path Path,
) -> anyhow::Result<(Middleware, &'path str, PathBuf)> {
    let dir_name = dir_path
        .file_name()
        .expect("Could not extract directory name")
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("File name was not valid UTF-8: {}", dir_path.display()))?;

    static INIT_PATHS: OnceLock<Vec<(Middleware, &str)>> = OnceLock::new();
    let order = INIT_PATHS.get_or_init(|| {
        vec![
            (Middleware::Project, "default.project.json"),
            (Middleware::ModuleScriptDir, "init.luau"),
            (Middleware::ModuleScriptDir, "init.lua"),
            (Middleware::ServerScriptDir, "init.server.luau"),
            (Middleware::ServerScriptDir, "init.server.lua"),
            (Middleware::ClientScriptDir, "init.client.luau"),
            (Middleware::ClientScriptDir, "init.client.lua"),
            (Middleware::CsvDir, "init.csv"),
        ]
    });

    for (middleware, name) in order {
        let test_path = dir_path.join(name);
        if vfs.metadata(&test_path).with_not_found()?.is_some() {
            return Ok((*middleware, dir_name, test_path));
        }
    }

    Ok((Middleware::Dir, dir_name, dir_path.to_path_buf()))
}

/// Gets a snapshot for a path given an InstanceContext and Vfs, taking
/// user specified sync rules into account.
fn snapshot_from_path(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    if let Some(rule) = context.get_user_sync_rule(path) {
        return rule
            .middleware
            .snapshot(context, vfs, path, rule.file_name_for_path(path)?);
    } else {
        for rule in default_sync_rules() {
            if rule.matches(path) {
                return rule.middleware.snapshot(
                    context,
                    vfs,
                    path,
                    rule.file_name_for_path(path)?,
                );
            }
        }
    }
    Ok(None)
}

/// Represents a possible 'transformer' used by Rojo to turn a file system
/// item into a Roblox Instance. Missing from this list is metadata.
/// This is deliberate, as metadata is not a snapshot middleware.
///
/// Directories cannot be used for sync rules so they're ignored by Serde.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Middleware {
    Csv,
    JsonModel,
    Json,
    ServerScript,
    ClientScript,
    ModuleScript,
    Project,
    Rbxm,
    Rbxmx,
    Toml,
    Text,
    Ignore,

    #[serde(skip_deserializing)]
    Dir,
    #[serde(skip_deserializing)]
    ServerScriptDir,
    #[serde(skip_deserializing)]
    ClientScriptDir,
    #[serde(skip_deserializing)]
    ModuleScriptDir,
    #[serde(skip_deserializing)]
    CsvDir,
}

impl Middleware {
    /// Creates a snapshot for the given path from the Middleware with
    /// the provided name.
    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
        name: &str,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let mut output = match self {
            Self::Csv => snapshot_csv(context, vfs, path, name),
            Self::JsonModel => snapshot_json_model(context, vfs, path, name),
            Self::Json => snapshot_json(context, vfs, path, name),
            Self::ServerScript => snapshot_lua(context, vfs, path, name, ScriptType::Server),
            Self::ClientScript => snapshot_lua(context, vfs, path, name, ScriptType::Client),
            Self::ModuleScript => snapshot_lua(context, vfs, path, name, ScriptType::Module),
            Self::Project => snapshot_project(context, vfs, path, name),
            Self::Rbxm => snapshot_rbxm(context, vfs, path, name),
            Self::Rbxmx => snapshot_rbxmx(context, vfs, path, name),
            Self::Toml => snapshot_toml(context, vfs, path, name),
            Self::Text => snapshot_txt(context, vfs, path, name),
            Self::Ignore => Ok(None),

            Self::Dir => snapshot_dir(context, vfs, path, name),
            Self::ServerScriptDir => {
                snapshot_lua_init(context, vfs, path, name, ScriptType::Server)
            }
            Self::ClientScriptDir => {
                snapshot_lua_init(context, vfs, path, name, ScriptType::Client)
            }
            Self::ModuleScriptDir => {
                snapshot_lua_init(context, vfs, path, name, ScriptType::Module)
            }
            Self::CsvDir => snapshot_csv_init(context, vfs, path, name),
        };
        if let Ok(Some(ref mut snapshot)) = output {
            snapshot.metadata.middleware = Some(*self);
        }
        output
    }

    /// Runs the syncback mechanism for the provided middleware given a
    /// SyncbackSnapshot.
    pub fn syncback<'sync>(
        &self,
        snapshot: &SyncbackSnapshot<'sync>,
    ) -> anyhow::Result<SyncbackReturn<'sync>> {
        let file_name = snapshot.path.file_name().and_then(|s| s.to_str());
        if let Some(file_name) = file_name {
            validate_file_name(file_name).with_context(|| {
                format!("cannot create a file or directory with name {file_name}")
            })?;
        }
        match self {
            Middleware::Csv => syncback_csv(snapshot),
            Middleware::JsonModel => syncback_json_model(snapshot),
            Middleware::Json => anyhow::bail!("cannot syncback Json middleware"),
            // Projects are only generated from files that already exist on the
            // file system, so we don't need to pass a file name.
            Middleware::Project => syncback_project(snapshot),
            Middleware::ServerScript => syncback_lua(snapshot),
            Middleware::ClientScript => syncback_lua(snapshot),
            Middleware::ModuleScript => syncback_lua(snapshot),
            Middleware::Rbxm => syncback_rbxm(snapshot),
            Middleware::Rbxmx => syncback_rbxmx(snapshot),
            Middleware::Toml => anyhow::bail!("cannot syncback Toml middleware"),
            Middleware::Text => syncback_txt(snapshot),
            Middleware::Ignore => anyhow::bail!("cannot syncback Ignore middleware"),
            Middleware::Dir => syncback_dir(snapshot),
            Middleware::ServerScriptDir => syncback_lua_init(ScriptType::Server, snapshot),
            Middleware::ClientScriptDir => syncback_lua_init(ScriptType::Client, snapshot),
            Middleware::ModuleScriptDir => syncback_lua_init(ScriptType::Module, snapshot),
            Middleware::CsvDir => syncback_csv_init(snapshot),
        }
    }

    /// Returns whether this particular middleware would become a directory.
    #[inline]
    pub fn is_dir(&self) -> bool {
        matches!(
            self,
            Middleware::Dir
                | Middleware::ServerScriptDir
                | Middleware::ClientScriptDir
                | Middleware::ModuleScriptDir
                | Middleware::CsvDir
        )
    }

    /// Returns whether this particular middleware sets its own properties.
    /// This applies to things like `JsonModel` and `Project`, since they
    /// set properties without needing a meta.json file.
    ///
    /// It does not cover middleware like `ServerScript` or `Csv` because they
    /// need a meta.json file to set properties that aren't their designated
    /// 'special' properties.
    #[inline]
    pub fn handles_own_properties(&self) -> bool {
        matches!(
            self,
            Middleware::JsonModel | Middleware::Project | Middleware::Rbxm | Middleware::Rbxmx
        )
    }

    /// Attempts to return a middleware that should be used for the given path.
    ///
    /// Returns `Err` only if the Vfs cannot read information about the path.
    pub fn middleware_for_path(
        vfs: &Vfs,
        sync_rules: &[SyncRule],
        path: &Path,
    ) -> anyhow::Result<Option<Self>> {
        let meta = match vfs.metadata(path).with_not_found()? {
            Some(meta) => meta,
            None => return Ok(None),
        };

        if meta.is_dir() {
            let (middleware, _, _) = get_dir_middleware(vfs, path)?;
            Ok(Some(middleware))
        } else {
            for rule in sync_rules.iter().chain(default_sync_rules()) {
                if rule.matches(path) {
                    return Ok(Some(rule.middleware));
                }
            }
            Ok(None)
        }
    }
}

/// A helper for easily defining a SyncRule. Arguments are passed literally
/// to this macro in the order `include`, `middleware`, `suffix`,
/// and `exclude`. Both `suffix` and `exclude` are optional.
///
/// All arguments except `middleware` are expected to be strings.
/// The `middleware` parameter is expected to be a variant of `Middleware`,
/// not including the enum name itself.
macro_rules! sync_rule {
    ($pattern:expr, $middleware:ident) => {
        SyncRule {
            middleware: Middleware::$middleware,
            include: Glob::new($pattern).unwrap(),
            exclude: None,
            suffix: None,
            base_path: PathBuf::new(),
        }
    };
    ($pattern:expr, $middleware:ident, $suffix:expr) => {
        SyncRule {
            middleware: Middleware::$middleware,
            include: Glob::new($pattern).unwrap(),
            exclude: None,
            suffix: Some($suffix.into()),
            base_path: PathBuf::new(),
        }
    };
    ($pattern:expr, $middleware:ident, $suffix:expr, $exclude:expr) => {
        SyncRule {
            middleware: Middleware::$middleware,
            include: Glob::new($pattern).unwrap(),
            exclude: Some(Glob::new($exclude).unwrap()),
            suffix: Some($suffix.into()),
            base_path: PathBuf::new(),
        }
    };
}

/// Defines the 'default' syncing rules that Rojo uses.
/// These do not broadly overlap, but the order matters for some in the case of
/// e.g. JSON models.
pub fn default_sync_rules() -> &'static [SyncRule] {
    static DEFAULT_SYNC_RULES: OnceLock<Vec<SyncRule>> = OnceLock::new();

    DEFAULT_SYNC_RULES.get_or_init(|| {
        vec![
            sync_rule!("*.server.lua", ServerScript, ".server.lua"),
            sync_rule!("*.server.luau", ServerScript, ".server.luau"),
            sync_rule!("*.client.lua", ClientScript, ".client.lua"),
            sync_rule!("*.client.luau", ClientScript, ".client.luau"),
            sync_rule!("*.{lua,luau}", ModuleScript),
            sync_rule!("*.project.json", Project, ".project.json"),
            sync_rule!("*.model.json", JsonModel, ".model.json"),
            sync_rule!("*.json", Json, ".json", "*.meta.json"),
            sync_rule!("*.toml", Toml),
            sync_rule!("*.csv", Csv),
            sync_rule!("*.txt", Text),
            sync_rule!("*.rbxmx", Rbxmx),
            sync_rule!("*.rbxm", Rbxm),
        ]
    })
}

/// Generates and returns two maps of unresolved properties and attributes,
/// respectively, appropriate for producing JSON or other file representations
/// that use unresolved variants. This function ignores any properties that are
/// at their default values. It also ignores any reserved attributes (i.e. ones
/// with names starting with `RBX`).
fn populate_unresolved_properties<'prop, I>(
    snapshot: &SyncbackSnapshot,
    new_inst: &Instance,
    properties: I,
) -> (
    BTreeMap<String, UnresolvedValue>,
    BTreeMap<String, UnresolvedValue>,
)
where
    I: IntoIterator<Item = (&'prop str, &'prop Variant)>,
{
    let db = rbx_reflection_database::get();
    let class_descriptor = db.classes.get(new_inst.class.as_str());
    let mut new_properties = BTreeMap::new();
    let mut new_attributes = BTreeMap::new();

    for (name, value) in properties {
        match value {
            Variant::Attributes(attrs) => {
                for (attr_name, attr_value) in attrs.iter() {
                    // We (probably) don't want to preserve internal attributes,
                    // only user defined ones.
                    if attr_name.starts_with("RBX") {
                        continue;
                    }
                    new_attributes.insert(
                        attr_name.clone(),
                        UnresolvedValue::from_variant_unambiguous(attr_value.clone()),
                    );
                }
            }
            Variant::SharedString(_) => {
                log::warn!(
                "Rojo cannot serialize the property {}.{name} in JSON files.\n\
                 If this is not acceptable, resave the Instance at '{}' manually as an RBXM or RBXMX.",
                new_inst.class, snapshot.get_new_inst_path(new_inst.referent())
            )
            }
            _ => {
                let new_prop_is_default = if let Some(class_descriptor) = class_descriptor {
                    if let Some(default) = db.find_default_property(class_descriptor, name) {
                        variant_eq(value, default)
                    } else {
                        false
                    }
                } else {
                    false
                };

                if !new_prop_is_default {
                    new_properties.insert(
                        name.to_owned(),
                        UnresolvedValue::from_variant(value.clone(), &new_inst.class, name),
                    );
                }
            }
        }
    }

    (new_properties, new_attributes)
}

fn node_should_reserialize(
    node_properties: &BTreeMap<String, UnresolvedValue>,
    node_attributes: &BTreeMap<String, UnresolvedValue>,
    instance: InstanceWithMeta,
) -> anyhow::Result<bool> {
    for (prop_name, unresolved_node_value) in node_properties {
        if let Some(inst_value) = instance.properties().get(prop_name) {
            let node_value = unresolved_node_value
                .clone()
                .resolve(instance.class_name(), prop_name)?;
            if !variant_eq(inst_value, &node_value) {
                return Ok(true);
            }
        } else {
            return Ok(true);
        }
    }

    // At this point, we know that the old instance contains at least all the
    // properties specified by the new node, and that none of the properties
    // specified by the new node differ from their old values.
    //
    // Because node properties at default values are represented by omission, we
    // still need to determine whether any properties missing from the new node
    // are at non-default values on the old instance. Otherwise, we will fail to
    // reserialize the node when properties change from non-default values to
    // default values.
    let db = rbx_reflection_database::get();
    let maybe_class_descriptor = db.classes.get(instance.class_name());
    for (prop_name, inst_value) in instance.properties() {
        // We skip attributes because they are compared later in this function.
        if prop_name == "Attributes" {
            continue;
        }

        // If the new node contains this property, then further checks are
        // unnecessary, as we've already compared such properties in the
        // previous loop.
        if node_properties.contains_key(prop_name) {
            continue;
        }

        // If a class descriptor cannot be found, then the reflection database
        // might be out of date.
        //
        // We should always reserialize the node in this case, otherwise we may
        // fail to reproduce properties of classes unknown to the reflection
        // database.
        let Some(class_descriptor) = maybe_class_descriptor else {
            return Ok(true);
        };

        // If a default value for this property cannot be found, then the
        // reflection database might be out of date, or this property simply
        // does not have a default value.
        //
        // We should always reserialize the node in this case, otherwise we may
        // fail to reproduce properties that do not have defaults in the
        // reflection database.
        let Some(default_value) = db.find_default_property(class_descriptor, prop_name) else {
            return Ok(true);
        };

        // If the old value for this property is non-default, and the new node does
        // not specify this property, it means that its value has changed to the
        // default, and the new node must be reserialized.
        if !variant_eq(inst_value, default_value) {
            return Ok(true);
        }
    }

    match instance.properties().get("Attributes") {
        Some(Variant::Attributes(inst_attributes)) => {
            // This will also catch if one is empty but the other isn't
            if node_attributes.len() != inst_attributes.len() {
                Ok(true)
            } else {
                for (attr_name, unresolved_node_value) in node_attributes {
                    if let Some(inst_value) = inst_attributes.get(attr_name.as_str()) {
                        let node_value = unresolved_node_value.clone().resolve_unambiguous()?;
                        if !variant_eq(inst_value, &node_value) {
                            return Ok(true);
                        }
                    } else {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
        Some(_) => Ok(true),
        None => {
            if !node_attributes.is_empty() {
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }
}

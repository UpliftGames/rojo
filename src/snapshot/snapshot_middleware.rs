use std::{any::Any, fmt::Debug, path::Path, sync::Arc};

use dyn_eq::DynEq;
use memofs::Vfs;
use rbx_dom_weak::{types::Ref, Instance, WeakDom};

use crate::snapshot_middleware::get_middlewares;

use super::{diff::DeepDiff, InstanceContext, InstanceMetadata, InstanceSnapshot, RojoTree};

pub const PRIORITY_MODEL_DIRECTORY: i32 = 80;
pub const PRIORITY_MODEL_JSON: i32 = 81;
pub const PRIORITY_MODEL_XML: i32 = 82;
pub const PRIORITY_MODEL_BINARY: i32 = 83;

pub const PRIORITY_DIRECTORY_CHECK_FALLBACK: i32 = 99;
pub const PRIORITY_SINGLE_READABLE: i32 = 100;
pub const PRIORITY_MANY_READABLE: i32 = 200;

pub const PRIORITY_ALWAYS: i32 = 1000;

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct SnapshotOverride {
    pub known_class: Option<String>,
}

pub trait SnapshotOverrideTrait {
    fn known_class_or(&self, def: &'static str) -> &str;
}

impl SnapshotOverrideTrait for SnapshotOverride {
    fn known_class_or(&self, def: &'static str) -> &str {
        self.known_class
            .as_ref()
            .map_or_else(|| def, |c| c.as_str())
    }
}

impl SnapshotOverrideTrait for Option<SnapshotOverride> {
    fn known_class_or(&self, def: &'static str) -> &str {
        self.as_ref().map(|o| o.known_class_or(def)).unwrap_or(def)
    }
}

pub trait SnapshotMiddleware: Debug + DynEq + Sync + Send {
    fn middleware_id(&self) -> &'static str;

    /// Default globs that should match this snapshot type.
    fn default_globs(&self) -> &[&'static str];

    fn exclude_globs(&self) -> &[&'static str] {
        &[]
    }

    fn match_only_directories(&self) -> bool {
        false
    }

    /// Name to search for when looking for "init" files, which turn a directory
    /// into a specific instance type instead of a folder.
    fn init_names(&self) -> &[&'static str];

    /// Creates a snapshot of the given instance.
    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>>;

    fn syncback_serializes_children(&self) -> bool {
        false
    }

    /// Priority/preference of using this syncback method to store the given
    /// instance.
    fn syncback_priority(
        &self,
        dom: &WeakDom,
        instance: &Instance,
        consider_descendants: bool,
    ) -> Option<i32>;

    /// Syncs an instance back to the filesystem, updating the existing files
    /// without removing them first.
    fn syncback_update(
        &self,
        vfs: &Vfs,
        path: &Path,
        diff: &DeepDiff,
        tree: &mut RojoTree,
        old_ref: Ref,
        new_dom: &WeakDom,
        instance_context: &InstanceContext,
        middleware_context: Option<Arc<dyn MiddlewareContextAny>>,
        overrides: Option<SnapshotOverride>,
    ) -> anyhow::Result<InstanceMetadata>;

    /// Syncs an instance back into the filesystem, creating new files.
    fn syncback_new(
        &self,
        vfs: &Vfs,
        parent_path: &Path,
        name: &str,
        new_dom: &WeakDom,
        new_ref: Ref,
        context: &InstanceContext,
        overrides: Option<SnapshotOverride>,
    ) -> anyhow::Result<Option<InstanceSnapshot>>;

    /// Destroys the filesystem representation of an instance.
    fn syncback_destroy(
        &self,
        vfs: &Vfs,
        path: &Path,
        tree: &mut RojoTree,
        old_ref: Ref,
    ) -> anyhow::Result<()>;
}
dyn_eq::eq_trait_object!(SnapshotMiddleware);

pub trait MiddlewareContextAny: Any + Debug + DynEq + Sync + Send + 'static {
    fn as_any_ref(self: &'_ Self) -> &'_ dyn Any;
    fn as_any_mut(self: &'_ mut Self) -> &'_ mut dyn Any;
    fn as_any_box(self: Box<Self>) -> Box<dyn Any>;
}
dyn_eq::eq_trait_object!(MiddlewareContextAny);

impl<T: Any + Debug + Eq + Send + Sync + 'static> MiddlewareContextAny for T {
    #[inline]
    fn as_any_ref(self: &'_ Self) -> &'_ dyn Any {
        self
    }

    #[inline]
    fn as_any_mut(self: &'_ mut Self) -> &'_ mut dyn Any {
        self
    }

    #[inline]
    fn as_any_box(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl dyn MiddlewareContextAny + 'static {
    #[inline]
    pub fn downcast_ref<T: 'static>(self: &'_ Self) -> Option<&'_ T> {
        self.as_any_ref().downcast_ref::<T>()
    }

    #[inline]
    pub fn downcast_mut<T: 'static>(self: &'_ mut Self) -> Option<&'_ mut T> {
        self.as_any_mut().downcast_mut::<T>()
    }
}

pub fn get_best_syncback_middleware_sorted(
    dom: &WeakDom,
    instance: &Instance,
    consider_descendants: bool,
    previous_middleware: Option<&'static str>,
) -> Option<Box<dyn Iterator<Item = &'static str>>> {
    if Some("project") == previous_middleware {
        return previous_middleware
            .map(|v| Box::new(std::iter::once(v)) as Box<dyn Iterator<Item = &'static str>>);
    }

    let mut middleware_candidates: Vec<(i32, &str)> = Vec::new();
    for (&middleware_id, middleware) in get_middlewares() {
        let priority = middleware.syncback_priority(dom, instance, consider_descendants);

        if let Some(priority) = priority {
            if let Some(previous_middleware_id) = previous_middleware {
                if middleware_id == previous_middleware_id {
                    return Some(Box::new(std::iter::once(middleware_id))
                        as Box<dyn Iterator<Item = &'static str>>);
                }
            }

            middleware_candidates.push((priority, middleware_id));
        }
    }

    middleware_candidates.sort_by_key(|(priority, _id)| -priority);

    Some(Box::new(
        middleware_candidates.into_iter().map(|(_priority, id)| id),
    ))
}

pub fn get_best_syncback_middleware(
    dom: &WeakDom,
    instance: &Instance,
    consider_descendants: bool,
    previous_middleware: Option<&'static str>,
) -> Option<&'static str> {
    get_best_syncback_middleware_sorted(dom, instance, consider_descendants, previous_middleware)
        .map(|mut iter| iter.next())
        .flatten()
}

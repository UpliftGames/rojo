use std::{
    any::Any,
    borrow::Cow,
    collections::HashSet,
    fmt::Debug,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use dyn_eq::DynEq;
use memofs::Vfs;
use rbx_dom_weak::{types::Ref, Instance, WeakDom};

use crate::snapshot_middleware::{get_middleware, get_middlewares};

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

    fn syncback_new_path(
        &self,
        parent_path: &Path,
        name: &str,
        new_inst: &Instance,
    ) -> anyhow::Result<PathBuf>;

    fn syncback(&self, sync: &SyncbackContextX<'_, '_>) -> anyhow::Result<SyncbackNode>;
}
dyn_eq::eq_trait_object!(SnapshotMiddleware);

pub struct SyncbackContextX<'old, 'new> {
    pub vfs: &'old Vfs,
    pub diff: &'new DeepDiff,
    pub path: &'new Path,
    pub old: Option<(&'old RojoTree, Ref, Option<MiddlewareContextArc>)>,
    pub new: (&'new WeakDom, Ref),
    pub metadata: &'new InstanceMetadata,
    pub overrides: Option<SnapshotOverride>,
}

impl SyncbackContextX<'_, '_> {
    pub fn ref_for_save(&self) -> Ref {
        self.old.opt_id().unwrap_or(self.new.id())
    }
    pub fn ref_for_save_if_used(&self) -> Option<Ref> {
        if self.diff.is_ref_used_in_property(self.new.id()) {
            return Some(self.new.id());
        }
        if let Some(old_ref) = self.old.opt_id() {
            if self.diff.is_ref_used_in_property(old_ref) {
                return Some(old_ref);
            }
        }
        None
    }
}

impl Clone for SyncbackContextX<'_, '_> {
    fn clone(&self) -> Self {
        Self {
            vfs: self.vfs,
            diff: self.diff,
            path: self.path,
            old: self
                .old
                .as_ref()
                .map(|(dom, id, ctx)| (*dom, *id, ctx.clone())),
            new: (self.new.0, self.new.1),
            metadata: self.metadata,
            overrides: self.overrides.clone(),
        }
    }
}

pub struct RefPair {
    pub old_ref: Option<Ref>,
    pub new_ref: Ref,
}

impl Into<RefPair> for Ref {
    fn into(self) -> RefPair {
        RefPair {
            old_ref: None,
            new_ref: self,
        }
    }
}

impl Into<RefPair> for (Ref, Ref) {
    fn into(self) -> RefPair {
        RefPair {
            old_ref: Some(self.0),
            new_ref: self.1,
        }
    }
}

impl Into<RefPair> for (Option<Ref>, Ref) {
    fn into(self) -> RefPair {
        RefPair {
            old_ref: self.0,
            new_ref: self.1,
        }
    }
}

pub trait OptOldTuple {
    fn opt_id(&self) -> Option<Ref>;
    fn opt_dom(&self) -> Option<&RojoTree>;
    fn opt_middleware_context(&self) -> Option<MiddlewareContextArc>;
}

impl OptOldTuple for Option<(&RojoTree, Ref, Option<MiddlewareContextArc>)> {
    fn opt_id(&self) -> Option<Ref> {
        self.as_ref().map(|(_, v, _)| *v)
    }
    fn opt_dom(&self) -> Option<&RojoTree> {
        match self.as_ref() {
            Some((tree, _, _)) => Some(*tree),
            None => None,
        }
    }
    fn opt_middleware_context(&self) -> Option<MiddlewareContextArc> {
        self.as_ref().map(|(_, _, v)| v.clone()).flatten()
    }
}

impl OptOldTuple for (&RojoTree, Ref, Option<MiddlewareContextArc>) {
    fn opt_id(&self) -> Option<Ref> {
        Some(self.1)
    }
    fn opt_dom(&self) -> Option<&RojoTree> {
        Some(&self.0)
    }
    fn opt_middleware_context(&self) -> Option<MiddlewareContextArc> {
        self.2.clone()
    }
}

pub trait OldTuple {
    fn id(&self) -> Ref;
    fn dom(&self) -> &RojoTree;
    fn middleware_context(&self) -> Option<MiddlewareContextArc>;
}

impl OldTuple for (&RojoTree, Ref, Option<MiddlewareContextArc>) {
    fn id(&self) -> Ref {
        self.1
    }
    fn dom(&self) -> &RojoTree {
        &self.0
    }
    fn middleware_context(&self) -> Option<MiddlewareContextArc> {
        self.2.clone()
    }
}

pub trait NewTuple {
    fn id(&self) -> Ref;
    fn dom(&self) -> &WeakDom;
}

impl NewTuple for (&WeakDom, Ref) {
    fn id(&self) -> Ref {
        self.1
    }
    fn dom(&self) -> &WeakDom {
        &self.0
    }
}

pub type GetChildren =
    Box<dyn FnOnce(&SyncbackContextX<'_, '_>) -> anyhow::Result<(Vec<SyncbackNode>, HashSet<Ref>)>>;

pub struct SyncbackNode {
    pub old_ref: Option<Ref>,
    pub new_ref: Ref,
    pub parent_ref: Option<Ref>,
    pub instance_snapshot: InstanceSnapshot,
    pub get_children: Option<GetChildren>,
    pub use_snapshot_children: bool,
    pub path: PathBuf,
}

impl SyncbackNode {
    pub fn new(refs: impl Into<RefPair>, path: &Path, instance_snapshot: InstanceSnapshot) -> Self {
        let refs: RefPair = refs.into();
        Self {
            old_ref: refs.old_ref,
            new_ref: refs.new_ref,
            parent_ref: None,
            instance_snapshot,
            get_children: None,
            use_snapshot_children: false,
            path: path.to_path_buf(),
        }
    }
    pub fn with_children(
        self,
        get_children: impl FnOnce(&SyncbackContextX<'_, '_>) -> anyhow::Result<(Vec<SyncbackNode>, HashSet<Ref>)>
            + 'static,
    ) -> Self {
        Self {
            get_children: Some(Box::new(get_children)),
            ..self
        }
    }
    pub fn use_snapshot_children(self) -> Self {
        Self {
            use_snapshot_children: true,
            ..self
        }
    }
}

pub trait SyncbackPlannerWrapped<'old, 'new> {
    fn syncback(
        &self,
        vfs: &Vfs,
        diff: &DeepDiff,
        overrides: Option<SnapshotOverride>,
    ) -> anyhow::Result<Option<SyncbackNode>>;
}

impl<'old, 'new> SyncbackPlannerWrapped<'old, 'new> for Option<SyncbackPlanner<'old, 'new>> {
    fn syncback(
        &self,
        vfs: &Vfs,
        diff: &DeepDiff,
        overrides: Option<SnapshotOverride>,
    ) -> anyhow::Result<Option<SyncbackNode>> {
        match self {
            Some(planner) => planner.syncback(vfs, diff, overrides).map(Some),
            None => Ok(None),
        }
    }
}
pub struct SyncbackPlanner<'old, 'new> {
    pub middleware_id: &'static str,
    pub path: Cow<'old, Path>,
    pub old: Option<(&'old RojoTree, Ref, Option<MiddlewareContextArc>)>,
    pub delete_old: Option<Ref>,
    pub new: (&'new WeakDom, Ref),
    pub metadata: Option<&'old InstanceMetadata>,
}

impl<'old, 'new> SyncbackPlanner<'old, 'new> {
    pub fn syncback(
        &self,
        vfs: &Vfs,
        diff: &DeepDiff,
        overrides: Option<SnapshotOverride>,
    ) -> anyhow::Result<SyncbackNode> {
        let new_meta;
        let metadata = match self.metadata {
            Some(metadata) => metadata,
            None => match self.old {
                Some((old_dom, old_ref, _)) => {
                    new_meta = InstanceMetadata::new()
                        .context(&old_dom.get_metadata(old_ref).unwrap().context);
                    &new_meta
                }
                None => {
                    new_meta = InstanceMetadata::new();
                    &new_meta
                }
            },
        };

        let mut result = get_middleware(self.middleware_id).syncback(&SyncbackContextX {
            vfs: vfs,
            diff: diff,
            path: &self.path,
            old: match &self.old {
                Some((old_dom, old_ref, old_middleware_context)) => {
                    Some((old_dom, *old_ref, old_middleware_context.clone()))
                }
                None => None,
            },
            new: self.new,
            metadata: metadata,
            overrides: overrides,
        });
        if let Some(delete_old) = self.delete_old {
            result = result.map(|mut v| {
                v.old_ref = Some(delete_old);
                v
            })
        }
        result
    }

    pub fn from_update(
        old_dom: &'old RojoTree,
        old_ref: Ref,
        new_dom: &'new WeakDom,
        new_ref: Ref,
        path: Option<&'old Path>,
        old_middleware: Option<(&'static str, Option<MiddlewareContextArc>)>,
    ) -> anyhow::Result<Option<SyncbackPlanner<'old, 'new>>> {
        let old_inst = old_dom
            .get_instance(old_ref)
            .with_context(|| "missing ref")?;
        let (old_middleware_id, old_middleware_context) = match old_middleware {
            Some((middleware_id, middleware_context)) => (Some(middleware_id), middleware_context),
            None => (
                old_inst.metadata().middleware_id,
                old_inst.metadata().middleware_context.clone(),
            ),
        };

        let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;
        let middleware_id =
            get_best_syncback_middleware(new_dom, new_inst, true, old_middleware_id);
        let middleware_id = match middleware_id {
            Some(v) => v,
            None => return Ok(None),
        };

        let path = match path {
            Some(path) => Cow::Borrowed(path),
            None => old_inst
                .metadata()
                .snapshot_source_path(true)
                .with_context(|| format!("missing path for {}", old_inst.name()))?,
        };

        if Some(middleware_id) == old_middleware_id {
            Ok(Some(SyncbackPlanner {
                middleware_id,
                path: path,
                old: Some((old_dom, old_ref, old_middleware_context)),
                new: (new_dom, new_ref),
                delete_old: None,
                metadata: Some(old_inst.metadata()),
            }))
        } else {
            let parent_path = path.parent().unwrap_or_else(|| Path::new("."));
            Self::from_new(parent_path, new_dom, new_ref).map(|v| {
                v.map(|mut v| {
                    v.delete_old = Some(old_ref);
                    v
                })
            })
        }
    }

    pub fn from_new(
        parent_path: &Path,
        new_dom: &'new WeakDom,
        new_ref: Ref,
    ) -> anyhow::Result<Option<SyncbackPlanner<'old, 'new>>> {
        let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;
        let middleware_id = get_best_syncback_middleware(new_dom, new_inst, true, None);
        let middleware_id = match middleware_id {
            Some(v) => v,
            None => return Ok(None),
        };

        let path = get_middleware(middleware_id).syncback_new_path(
            parent_path,
            &new_inst.name,
            new_inst,
        )?;

        Ok(Some(SyncbackPlanner {
            middleware_id,
            path: Cow::Owned(path),
            old: None,
            delete_old: None,
            new: (new_dom, new_ref),
            metadata: None,
        }))
    }
}

pub trait MiddlewareContextAny: Any + Debug + DynEq + Sync + Send + 'static {
    fn as_any_ref(self: &'_ Self) -> &'_ dyn Any;
    fn as_any_mut(self: &'_ mut Self) -> &'_ mut dyn Any;
    fn as_any_box(self: Box<Self>) -> Box<dyn Any>;
}
dyn_eq::eq_trait_object!(MiddlewareContextAny);

pub type MiddlewareContextArc = Arc<dyn MiddlewareContextAny>;

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

pub fn get_best_syncback_middleware_must_not_serialize_children(
    dom: &WeakDom,
    instance: &Instance,
    consider_descendants: bool,
    previous_middleware: Option<&'static str>,
) -> Option<&'static str> {
    get_best_syncback_middleware_sorted(dom, instance, consider_descendants, previous_middleware)
        .map(|mut iter| iter.find(|&id| !get_middleware(id).syncback_serializes_children()))
        .flatten()
}

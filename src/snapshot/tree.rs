use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use memofs::Vfs;
use rbx_dom_weak::{
    types::{Ref, Variant},
    Instance, InstanceBuilder, WeakDom,
};

use crate::{multimap::MultiMap, snapshot_middleware::get_middlewares};

use super::{
    diff::DeepDiff, get_best_syncback_middleware, InstanceContext, InstanceMetadata,
    InstanceSnapshot, InstigatingSource,
};

pub enum SyncbackTarget {
    Replace(Ref),
    NewParentedTo(Ref),
}

/// An expanded variant of rbx_dom_weak's `WeakDom` that tracks additional
/// metadata per instance that's Rojo-specific.
///
/// This tree is also optimized for doing fast incremental updates and patches.
#[derive(Debug)]
pub struct RojoTree {
    /// Contains the instances without their Rojo-specific metadata.
    inner: WeakDom,

    /// Metadata associated with each instance that is kept up-to-date with the
    /// set of actual instances.
    metadata_map: HashMap<Ref, InstanceMetadata>,

    /// A multimap from source paths to all of the root instances that were
    /// constructed from that path.
    ///
    /// Descendants of those instances should not be contained in the set, the
    /// value portion of the map is also a set in order to support the same path
    /// appearing multiple times in the same Rojo project. This is sometimes
    /// called "path aliasing" in various Rojo documentation.
    path_to_ids: MultiMap<PathBuf, Ref>,
}

impl RojoTree {
    pub fn new(snapshot: InstanceSnapshot) -> RojoTree {
        let root_builder = InstanceBuilder::new(snapshot.class_name.to_owned())
            .with_name(snapshot.name.to_owned())
            .with_properties(snapshot.properties);

        let mut tree = RojoTree {
            inner: WeakDom::new(root_builder),
            metadata_map: HashMap::new(),
            path_to_ids: MultiMap::new(),
        };

        let root_ref = tree.inner.root_ref();

        tree.insert_metadata(root_ref, snapshot.metadata);

        for child in snapshot.children {
            tree.insert_instance(root_ref, child);
        }

        tree
    }

    pub fn inner(&self) -> &WeakDom {
        &self.inner
    }

    pub fn get_root_id(&self) -> Ref {
        self.inner.root_ref()
    }

    pub fn fix_unique_id_collisions(&mut self) {
        let inner = &mut self.inner;

        let mut processing = vec![inner.root_ref()];
        let mut ids: HashSet<String> = HashSet::new();

        while let Some(inst_ref) = processing.pop() {
            let inst = inner.get_by_ref_mut(inst_ref).unwrap();
            let keys = inst
                .properties
                .keys()
                .map(|k| k.clone())
                .collect::<Vec<_>>();
            for key in keys {
                let is_conflicting_unique_id = {
                    if let Variant::UniqueId(id) = inst.properties.get(&key).unwrap() {
                        !ids.insert(id.to_string())
                    } else {
                        false
                    }
                };
                if is_conflicting_unique_id {
                    inst.properties.remove(&key);
                }
            }
            processing.extend(inst.children());
        }
    }

    pub fn get_instance(&self, id: Ref) -> Option<InstanceWithMeta> {
        if let Some(instance) = self.inner.get_by_ref(id) {
            let metadata = self.metadata_map.get(&id).unwrap();

            Some(InstanceWithMeta { instance, metadata })
        } else {
            None
        }
    }

    pub fn get_instance_mut(&mut self, id: Ref) -> Option<InstanceWithMetaMut> {
        if let Some(instance) = self.inner.get_by_ref_mut(id) {
            let metadata = self.metadata_map.get_mut(&id).unwrap();

            Some(InstanceWithMetaMut { instance, metadata })
        } else {
            None
        }
    }

    pub fn insert_instance(&mut self, parent_ref: Ref, snapshot: InstanceSnapshot) -> Ref {
        let builder = InstanceBuilder::empty()
            .with_class(snapshot.class_name.into_owned())
            .with_name(snapshot.name.into_owned())
            .with_properties(snapshot.properties);

        let referent = self.inner.insert(parent_ref, builder);
        self.insert_metadata(referent, snapshot.metadata);

        for child in snapshot.children {
            self.insert_instance(referent, child);
        }

        referent
    }

    pub fn update_props(&mut self, id: Ref, instance: &Instance) {
        let inst = self.inner.get_by_ref_mut(id).unwrap();
        inst.class = instance.class.clone();
        inst.name = instance.name.clone();

        inst.properties.clear();
        inst.properties.extend(
            instance
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
    }

    pub fn remove(&mut self, id: Ref) {
        let mut to_move = VecDeque::new();
        to_move.push_back(id);

        while let Some(id) = to_move.pop_front() {
            self.remove_metadata(id);

            if let Some(instance) = self.inner.get_by_ref(id) {
                to_move.extend(instance.children().iter().copied());
            }
        }

        self.inner.destroy(id);
    }

    /// Replaces the metadata associated with the given instance ID.
    pub fn update_metadata(&mut self, id: Ref, metadata: InstanceMetadata) {
        use std::collections::hash_map::Entry;

        match self.metadata_map.entry(id) {
            Entry::Occupied(mut entry) => {
                let existing_metadata = entry.get();

                // If this instance's source path changed, we need to update our
                // path associations so that file changes will trigger updates
                // to this instance correctly.
                if existing_metadata.relevant_paths != metadata.relevant_paths {
                    for existing_path in &existing_metadata.relevant_paths {
                        self.path_to_ids.remove(existing_path, id);
                    }

                    for new_path in &metadata.relevant_paths {
                        self.path_to_ids.insert(new_path.clone(), id);
                    }
                }

                entry.insert(metadata);
            }
            Entry::Vacant(entry) => {
                entry.insert(metadata);
            }
        }
    }

    pub fn descendants(&self, id: Ref) -> RojoDescendants<'_> {
        let mut queue = VecDeque::new();
        queue.push_back(id);

        RojoDescendants { queue, tree: self }
    }

    pub fn get_ids_at_path(&self, path: &Path) -> &[Ref] {
        self.path_to_ids.get(path)
    }

    pub fn get_metadata(&self, id: Ref) -> Option<&InstanceMetadata> {
        self.metadata_map.get(&id)
    }

    fn insert_metadata(&mut self, id: Ref, metadata: InstanceMetadata) {
        for path in &metadata.relevant_paths {
            self.path_to_ids.insert(path.clone(), id);
        }

        self.metadata_map.insert(id, metadata);
    }

    /// Moves the Rojo metadata from the instance with the given ID from this
    /// tree into some loose maps.
    fn remove_metadata(&mut self, id: Ref) {
        let metadata = self.metadata_map.remove(&id).unwrap();

        for path in &metadata.relevant_paths {
            self.path_to_ids.remove(path, id);
        }
    }

    pub fn syncback_children(
        &mut self,
        vfs: &Vfs,
        diff: &DeepDiff,
        parent_id: Ref,
        parent_path: &Path,
        new_dom: &WeakDom,
        context: &InstanceContext,
    ) -> anyhow::Result<()> {
        log::trace!(
            "syncback_children for {} {}",
            self.get_instance(parent_id).unwrap().name(),
            parent_path.display()
        );
        let tree = self;
        let children = diff.get_children(tree.inner(), new_dom, parent_id);
        if let Some((added, removed, changed, _unchanged)) = children {
            for old_child_ref in changed {
                let new_child_ref = diff
                    .get_matching_new_ref(old_child_ref)
                    .with_context(|| "no matching new ref")?;
                let new_child_inst = new_dom.get_by_ref(new_child_ref).unwrap();

                let old_child_inst = tree.get_instance(old_child_ref).unwrap();
                let existing_middleware = old_child_inst.metadata().middleware_id;

                let child_context = old_child_inst.metadata().context.clone();

                let best_middleware = get_best_syncback_middleware(
                    tree.inner(),
                    new_child_inst,
                    false,
                    existing_middleware,
                )
                .with_context(|| "Cannot syncback instance")?;

                let existing_path = if let Some(_existing_middleware) = existing_middleware {
                    let existing_path = old_child_inst.metadata().snapshot_source_path();

                    let existing_path = match existing_path {
                        Some(path) => path.to_path_buf(),
                        None => {
                            log::trace!("missing path for {} (skipping)", old_child_inst.name());
                            continue;
                        }
                    };

                    Some(existing_path)
                } else {
                    None
                };

                if let Some(existing_path) = &existing_path {
                    if !child_context.should_syncback_path(existing_path) {
                        continue;
                    }
                }

                let existing_middleware_context =
                    old_child_inst.metadata().middleware_context.clone();

                if Some(best_middleware) == existing_middleware {
                    let existing_path = existing_path.unwrap();

                    let metadata = get_middlewares()[best_middleware].syncback_update(
                        vfs,
                        &existing_path,
                        diff,
                        tree,
                        old_child_ref,
                        new_dom,
                        &child_context,
                        existing_middleware_context,
                        None,
                    )?;
                    tree.update_metadata(old_child_ref, metadata);
                } else {
                    if let Some(existing_middleware) = existing_middleware {
                        let existing_path = existing_path.unwrap();

                        get_middlewares()[existing_middleware].syncback_destroy(
                            vfs,
                            &existing_path,
                            tree,
                            old_child_ref,
                        )?;
                    }
                    // reconstruct fs via syncback
                    let child_snapshot = get_middlewares()[best_middleware]
                        .syncback_new(
                            vfs,
                            parent_path,
                            &new_child_inst.name,
                            new_dom,
                            new_child_ref,
                            &child_context,
                            None,
                        )
                        .with_context(|| "failed to create instance on filesystem")?;

                    let child_snapshot = match child_snapshot {
                        Some(child_snapshot) => child_snapshot,
                        None => {
                            log::info!(
                                "Skipping {} because its path is excluded by ignore patterns",
                                new_child_inst.name()
                            );
                            continue;
                        }
                    };

                    tree.remove(old_child_ref);
                    tree.insert_instance(parent_id, child_snapshot);
                }
            }

            for old_child_ref in removed {
                let old_child_inst = tree.get_instance(old_child_ref).unwrap();
                let old_child_middleware = old_child_inst.metadata().middleware_id;
                if let Some(old_child_middleware) = old_child_middleware {
                    let old_child_path = old_child_inst
                        .metadata()
                        .snapshot_source_path()
                        .with_context(|| "missing path")?
                        .to_path_buf();

                    get_middlewares()[old_child_middleware]
                        .syncback_destroy(vfs, &old_child_path, tree, old_child_ref)
                        .with_context(|| "failed to destroy instance on filesystem")?;
                }

                tree.remove(old_child_ref);
            }

            if !tree
                .get_instance(parent_id)
                .unwrap()
                .metadata()
                .ignore_unknown_instances
            {
                for new_child_ref in added {
                    let new_child_inst = new_dom.get_by_ref(new_child_ref).unwrap();
                    let new_child_middleware =
                        get_best_syncback_middleware(new_dom, new_child_inst, true, None)
                            .with_context(|| "instance cannot be synced")?;
                    let child_snapshot = get_middlewares()[new_child_middleware]
                        .syncback_new(
                            vfs,
                            parent_path,
                            &new_child_inst.name,
                            new_dom,
                            new_child_ref,
                            context,
                            None,
                        )
                        .with_context(|| "failed to create instance on filesystem")?;

                    tree.insert_instance(parent_id, child_snapshot);
                }
            }
        }

        Ok(())
    }

    pub fn syncback(
        &mut self,
        vfs: &Vfs,
        old_id: Ref,
        new_dom: &WeakDom,
        new_id: Ref,
    ) -> anyhow::Result<()> {
        let empty_map = BTreeMap::new();

        let diff = DeepDiff::new(&self.inner, old_id, new_dom, new_id, |old_ref| {
            match self.get_metadata(old_ref) {
                Some(metadata) => &metadata.context.syncback.property_filters_diff,
                None => &empty_map,
            }
        });

        self.syncback_update(vfs, &diff, old_id, new_dom)
    }

    fn syncback_update(
        &mut self,
        vfs: &Vfs,
        diff: &DeepDiff,
        base_target: Ref,
        new_dom: &WeakDom,
    ) -> anyhow::Result<()> {
        let (old_inst, old_id, old_path) = {
            let mut syncable = self
                .get_instance(base_target)
                .with_context(|| "Missing ref")?;
            loop {
                log::trace!(
                    "might compare {} {:?} {:?}",
                    syncable.name(),
                    &syncable.metadata.middleware_id,
                    diff.get_matching_new_ref(syncable.id())
                );
                if syncable.metadata.middleware_id.is_some()
                    && diff.get_matching_new_ref(syncable.id()).is_some()
                {
                    log::trace!("checking {}", syncable.name());
                    log::trace!("  source: {:?}", syncable.metadata.instigating_source);
                    if let Some(InstigatingSource::Path(path)) =
                        &syncable.metadata.instigating_source
                    {
                        log::trace!("  is syncable");
                        break (syncable, syncable.id(), path.clone());
                        // NOTE: we skip all project nodes as we can't reconcile
                        // those.

                        // NOTE: The initial project node is a
                        // InstigatingSource::Path, it's _children_ are
                        // InstigatingSource::ProjectNode. In effect, we skip
                        // its direct children and go straight to the
                        // `.project.json` file itself.
                    }
                }

                syncable = match self.get_instance(syncable.parent()) {
                    Some(next_syncable) => next_syncable,
                    None => bail!("No syncable ancestor"),
                };
            }
        };

        let new_id = diff.get_matching_new_ref(old_id).unwrap();
        let new_inst = new_dom.get_by_ref(new_id).unwrap();

        let context = old_inst.metadata.context.clone();

        let old_middleware_id = old_inst.metadata.middleware_id.unwrap();
        let old_middleware_context = old_inst.metadata.middleware_context.clone();

        let new_middleware_id =
            get_best_syncback_middleware(new_dom, new_inst, true, Some(old_middleware_id));

        log::trace!("old_middleware_id: {:#?}", old_middleware_id);
        log::trace!("new_middleware_id: {:#?}", new_middleware_id);

        if new_middleware_id == Some(old_middleware_id) {
            log::trace!("updating");
            let new_middleware_id = new_middleware_id.unwrap();
            let metadata = get_middlewares()[new_middleware_id].syncback_update(
                vfs,
                &old_path,
                diff,
                self,
                old_id,
                new_dom,
                &context,
                old_middleware_context,
                None,
            )?;
            self.update_metadata(old_id, metadata);
            log::trace!("update complete");
        } else {
            get_middlewares()[old_middleware_id].syncback_destroy(vfs, &old_path, self, old_id)?;
            self.remove(old_id);

            if let Some(new_middleware_id) = new_middleware_id {
                let snapshot = get_middlewares()[new_middleware_id]
                    .syncback_new(
                        vfs,
                        &old_path,
                        &new_inst.name,
                        new_dom,
                        new_id,
                        &context,
                        None,
                    )
                    .with_context(|| "failed to create instance on filesystem")?;

                self.insert_instance(old_id, snapshot);
            }
        }

        Ok(())
    }
}

pub struct RojoDescendants<'a> {
    queue: VecDeque<Ref>,
    tree: &'a RojoTree,
}

impl<'a> Iterator for RojoDescendants<'a> {
    type Item = InstanceWithMeta<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.queue.pop_front()?;

        let instance = self
            .tree
            .inner
            .get_by_ref(id)
            .expect("Instance did not exist");

        let metadata = self
            .tree
            .get_metadata(instance.referent())
            .expect("Metadata did not exist for instance");

        self.queue.extend(instance.children().iter().copied());

        Some(InstanceWithMeta { instance, metadata })
    }
}

/// RojoTree's equivalent of `&'a Instance`.
///
/// This has to be a value type for RojoTree because the instance and metadata
/// are stored in different places. The mutable equivalent is
/// `InstanceWithMetaMut`.
#[derive(Debug, Clone, Copy)]
pub struct InstanceWithMeta<'a> {
    instance: &'a Instance,
    metadata: &'a InstanceMetadata,
}

impl<'a> InstanceWithMeta<'a> {
    pub fn id(&self) -> Ref {
        self.instance.referent()
    }

    pub fn parent(&self) -> Ref {
        self.instance.parent()
    }

    pub fn name(&self) -> &'a str {
        &self.instance.name
    }

    pub fn class_name(&self) -> &'a str {
        &self.instance.class
    }

    pub fn properties(&self) -> &'a HashMap<String, Variant> {
        &self.instance.properties
    }

    pub fn children(&self) -> &'a [Ref] {
        self.instance.children()
    }

    pub fn metadata(&self) -> &'a InstanceMetadata {
        &self.metadata
    }
}

/// RojoTree's equivalent of `&'a mut Instance`.
///
/// This has to be a value type for RojoTree because the instance and metadata
/// are stored in different places. The immutable equivalent is
/// `InstanceWithMeta`.
#[derive(Debug)]
pub struct InstanceWithMetaMut<'a> {
    instance: &'a mut Instance,
    metadata: &'a mut InstanceMetadata,
}

impl InstanceWithMetaMut<'_> {
    pub fn id(&self) -> Ref {
        self.instance.referent()
    }

    pub fn name(&self) -> &str {
        &self.instance.name
    }

    pub fn name_mut(&mut self) -> &mut String {
        &mut self.instance.name
    }

    pub fn class_name(&self) -> &str {
        &self.instance.class
    }

    pub fn class_name_mut(&mut self) -> &mut String {
        &mut self.instance.class
    }

    pub fn properties(&self) -> &HashMap<String, Variant> {
        &self.instance.properties
    }

    pub fn properties_mut(&mut self) -> &mut HashMap<String, Variant> {
        &mut self.instance.properties
    }

    pub fn children(&self) -> &[Ref] {
        self.instance.children()
    }

    pub fn metadata(&self) -> &InstanceMetadata {
        &self.metadata
    }
}

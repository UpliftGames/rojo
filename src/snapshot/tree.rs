use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{bail, Context};
use memofs::Vfs;
use rbx_dom_weak::{
    types::{Ref, Variant},
    Instance, InstanceBuilder, WeakDom,
};

use crate::{
    multimap::MultiMap,
    snapshot::{empty_hashset, InstigatingSource},
    snapshot_middleware::get_middleware,
};

use super::{
    diff::DeepDiff, DiffOptions, FsSnapshot, InstanceMetadata, InstanceSnapshot, PropertyFilter,
    SyncbackArgs, SyncbackNode,
};

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

    /// A map of preferred_ref -> actual ref
    ///
    /// This is temporarily filled during syncback then cleared once no longer
    /// needed.
    syncback_refs_map: HashMap<Ref, Ref>,
}

impl RojoTree {
    pub fn new(snapshot: InstanceSnapshot) -> RojoTree {
        let root_builder = InstanceBuilder::new(snapshot.class_name.into_owned())
            .with_name(snapshot.name.into_owned())
            .with_properties(snapshot.properties);

        let mut tree = RojoTree {
            inner: WeakDom::new(root_builder),
            metadata_map: HashMap::new(),
            path_to_ids: MultiMap::new(),
            syncback_refs_map: HashMap::new(),
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

    pub fn into_weakdom(self) -> WeakDom {
        self.inner
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
            let keys = inst.properties.keys().cloned().collect::<Vec<_>>();
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

    pub fn warn_for_broken_refs(&self) {
        for inst in self.descendants(self.get_root_id()) {
            for (key, value) in inst.properties().iter() {
                if let Variant::Ref(ref_id) = value {
                    if ref_id.is_some() && self.get_instance(*ref_id).is_none() {
                        let full_name = std::iter::successors(Some(inst), |inst| {
                            self.get_instance(inst.parent())
                        })
                        .map(|inst| inst.name())
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join(".");

                        log::warn!("Broken object reference property: {} of {}", key, full_name);
                    }
                }
            }
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
        let mut builder = InstanceBuilder::empty()
            .with_class(snapshot.class_name.into_owned())
            .with_name(snapshot.name.into_owned())
            .with_properties(snapshot.properties);

        if let Some(preferred_ref) = snapshot.preferred_ref {
            if self.inner.get_by_ref(preferred_ref).is_none() {
                builder.set_referent(preferred_ref);
            }
        }

        let referent = self.inner.insert(parent_ref, builder);
        self.insert_metadata(referent, snapshot.metadata);

        if let Some(preferred_ref) = snapshot.preferred_ref {
            self.syncback_refs_map.insert(preferred_ref, referent);
        }

        for child in snapshot.children {
            self.insert_instance(referent, child);
        }

        referent
    }

    pub fn update_props(&mut self, id: Ref, snapshot: InstanceSnapshot) {
        let inst = self.inner.get_by_ref_mut(id).unwrap();
        inst.class = snapshot.class_name.to_string();
        inst.name = snapshot.name.to_string();

        inst.properties.clear();
        inst.properties.extend(snapshot.properties);
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

    pub fn fix_referent_properties_post_syncback(&mut self, ref_map: HashMap<Ref, Ref>) {
        let mut processing = vec![self.inner.root_ref()];
        while let Some(inst_ref) = processing.pop() {
            let inst = self.inner.get_by_ref_mut(inst_ref).unwrap();
            inst.properties.iter_mut().for_each(|(_k, v)| {
                if let Variant::Ref(ref_id) = v {
                    if let Some(ref_id) = ref_map.get(ref_id) {
                        *v = Variant::Ref(*ref_id);
                    }
                }
            });
            processing.extend(inst.children());
        }
    }

    pub fn syncback_get_filters(&self, old_ref: Ref) -> &BTreeMap<String, PropertyFilter> {
        static EMPTY_MAP: OnceLock<BTreeMap<String, PropertyFilter>> = OnceLock::new();

        match self.get_metadata(old_ref) {
            Some(metadata) => &metadata.context.syncback.property_filters_diff,
            None => EMPTY_MAP.get_or_init(BTreeMap::new),
        }
    }

    pub fn syncback_should_skip(&self, old_ref: Ref) -> bool {
        if let Some(metadata) = self.get_metadata(old_ref) {
            if let Some(source_path) = metadata.snapshot_source_path(true) {
                return metadata
                    .context
                    .syncback
                    .as_ref()
                    .exclude_globs
                    .iter()
                    .any(|glob| glob.is_match(&source_path));
            }
        }

        false
    }

    pub fn syncback_get_skip_instance_names(&self, old_ref: Ref) -> &HashSet<String> {
        match self.get_metadata(old_ref) {
            Some(metadata) => &metadata.context.syncback.skip_instance_names,
            None => empty_hashset(),
        }
    }

    pub fn syncback_should_show_adds_removes(&self, old_ref: Ref) -> bool {
        if let Some(metadata) = self.get_metadata(old_ref) {
            // TODO: make this a method of the middleware trait
            metadata.middleware_id.map_or(true, |v| v != "project")
        } else {
            true
        }
    }

    pub fn syncback_start(
        &mut self,
        _vfs: &Vfs,
        old_id: Ref,
        new_dom: &mut WeakDom,
        new_id: Ref,
        diff_options: DiffOptions,
    ) -> DeepDiff {
        DeepDiff::new(
            &self.inner,
            old_id,
            new_dom,
            new_id,
            diff_options,
            |old_ref| self.syncback_get_filters(old_ref),
            |old_ref| self.syncback_should_skip(old_ref),
            |old_ref| self.syncback_get_skip_instance_names(old_ref),
        )
    }

    pub fn syncback_process(
        &mut self,
        vfs: &Vfs,
        diff: &DeepDiff,
        base_target: Ref,
        new_dom: &WeakDom,
    ) -> anyhow::Result<()> {
        let mut processing: Vec<SyncbackNode> = Vec::new();
        {
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

            let mut node = get_middleware(old_inst.metadata.middleware_id.unwrap()).syncback(
                &SyncbackArgs {
                    vfs,
                    diff,
                    path: &old_path,
                    old: Some((self, old_id, old_inst.metadata.middleware_context.clone())),
                    new: (new_dom, new_id),
                    metadata: old_inst.metadata,
                    overrides: None,
                },
            )?;

            node.parent_ref = Some(old_inst.parent());

            processing.push(node);
        }

        while let Some(item) = processing.pop() {
            let inst_snapshot = &item.instance_snapshot;
            if let Some(fs_snapshot) = &inst_snapshot.metadata.fs_snapshot {
                let violates_rules = fs_snapshot
                    .files
                    .keys()
                    .chain(fs_snapshot.dirs.iter())
                    .any(|path| !inst_snapshot.metadata.context.should_syncback_path(path));
                if violates_rules {
                    log::info!("Skipping syncback of {} because it is excluded by syncback ignore path rules.", inst_snapshot.name);
                    continue;
                }
            }

            let old_fs_snapshot = item
                .old_ref
                .and_then(|v| self.get_instance(v).unwrap().metadata.fs_snapshot.as_ref());

            FsSnapshot::reconcile(
                vfs,
                old_fs_snapshot,
                inst_snapshot.metadata.fs_snapshot.as_ref(),
            )?;

            let preferred_ref = inst_snapshot.preferred_ref;

            let insert_ref;
            let metadata;

            if let Some(old_ref) = item.old_ref {
                if item.use_snapshot_children {
                    self.remove(old_ref);
                    insert_ref =
                        self.insert_instance(item.parent_ref.unwrap(), item.instance_snapshot);
                    metadata = self.get_metadata(insert_ref).unwrap();
                } else {
                    self.update_props(old_ref, item.instance_snapshot);
                    insert_ref = old_ref;
                    metadata = self.get_metadata(old_ref).unwrap();
                }
            } else {
                insert_ref = self.insert_instance(item.parent_ref.unwrap(), item.instance_snapshot);
                metadata = self.get_metadata(insert_ref).unwrap();
            }

            if let Some(preferred_ref) = preferred_ref {
                if insert_ref != preferred_ref {
                    log::trace!(
                        "Item {} needed to have a new id generated for it.",
                        self.get_instance(insert_ref).unwrap().name()
                    );
                }
            }

            if item.old_ref.is_some() && item.use_snapshot_children {
                continue;
            }

            let path = item.path;
            let middleware_context = metadata.middleware_context.clone();

            if let Some(get_children) = item.get_children {
                let (children, removed) = get_children(&SyncbackArgs {
                    vfs,
                    diff,
                    path: &path,
                    old: item
                        .old_ref
                        .as_ref()
                        .map(|old_id| (&*self, *old_id, middleware_context)),
                    new: (new_dom, item.new_ref),
                    metadata,
                    overrides: None, // TODO
                })?;

                for mut child in children {
                    // TODO: fix this, it's wrong for projects
                    // dirs and projects can probably just do this themselves.
                    child.parent_ref = Some(insert_ref);
                    processing.push(child);
                }

                for id in removed {
                    let remove_metadata = self.get_metadata(id);
                    if let Some(remove_metadata) = remove_metadata {
                        if let Some(remove_fs_snapshot) = &remove_metadata.fs_snapshot {
                            FsSnapshot::reconcile(vfs, Some(remove_fs_snapshot), None)?;
                        }
                    }

                    self.remove(id);
                }
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
        self.metadata
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
        self.metadata
    }
}

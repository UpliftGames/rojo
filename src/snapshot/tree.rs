use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
};

use rbx_dom_weak::{
    types::{Ref, Variant},
    Instance, InstanceBuilder, WeakDom,
};
use serde::{Deserialize, Serialize};

use crate::multimap::MultiMap;

use super::{InstanceMetadata, InstanceSnapshot, REF_ID_ATTRIBUTE_NAME};

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

    /// A map of user-specified RojoRef properties to the internal ID for that
    /// Instance.
    user_ids_to_ids: HashMap<RojoRef, Ref>,
}

impl RojoTree {
    pub fn new(snapshot: InstanceSnapshot) -> RojoTree {
        let root_builder = InstanceBuilder::new(snapshot.class_name)
            .with_name(snapshot.name)
            .with_properties(snapshot.properties);

        let mut tree = RojoTree {
            inner: WeakDom::new(root_builder),
            metadata_map: HashMap::new(),
            path_to_ids: MultiMap::new(),
            user_ids_to_ids: HashMap::new(),
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

        if snapshot.metadata.specified_id.is_none() {
            let inst = self.inner.get_by_ref(referent).unwrap();
            for prop_value in inst.properties.values() {
                if let Variant::Attributes(attrs) = prop_value {
                    match attrs.get(REF_ID_ATTRIBUTE_NAME) {
                        Some(Variant::String(id)) => {
                            self.user_ids_to_ids.insert(id.clone().into(), referent);
                        }
                        Some(value) => log::warn!(
                            "Attribute {REF_ID_ATTRIBUTE_NAME} is of type {:?}\
                        when it was supposed to be String",
                            value.ty()
                        ),
                        None => {
                            self.user_ids_to_ids.insert(referent.into(), referent);
                        }
                    }
                    break;
                }
            }
        } else {
            self.user_ids_to_ids
                .insert(snapshot.metadata.specified_id.clone(), referent);
        }

        self.insert_metadata(referent, snapshot.metadata);

        for child in snapshot.children {
            self.insert_instance(referent, child);
        }

        referent
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

    /// If the provided `RojoRef` references an Instance in the tree, it is
    /// returned. Otherwise, returns `None`.
    #[inline]
    pub fn get_real_id(&self, id: &RojoRef) -> Option<Ref> {
        self.user_ids_to_ids.get(id).copied()
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

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub enum RojoRef {
    Custom(String),
    Ref(Ref),
}

impl RojoRef {
    #[inline]
    pub fn none() -> Self {
        RojoRef::Ref(Ref::none())
    }

    /// Returns whether this `RojoRef` is a custom one or a normal `Ref`.
    #[inline]
    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Returns whether this `RojoRef` represents a `None` referent.
    #[inline]
    pub fn is_none(&self) -> bool {
        match self {
            Self::Ref(inner) => inner.is_none(),
            _ => false,
        }
    }
}

impl From<String> for RojoRef {
    fn from(value: String) -> Self {
        Self::Custom(value)
    }
}

impl From<Ref> for RojoRef {
    fn from(value: Ref) -> Self {
        Self::Ref(value)
    }
}

impl Default for RojoRef {
    fn default() -> Self {
        Self::Ref(Ref::none())
    }
}

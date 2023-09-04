use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Display,
    iter::{Chain, FilterMap, Map},
    ops::AddAssign,
};

use float_cmp::approx_eq;
use itertools::{Itertools, Unique};
use rbx_dom_weak::{
    types::{Ref, Variant, Vector2, Vector3},
    Instance, WeakDom,
};

use super::{
    default_filters_diff, filter, get_default_property, PropertiesFiltered, PropertyFilter,
    ToVariantBinaryString, WeakDomExtra,
};

// We've copied the exact return type from the iterator here so that we can
// return it directly without collecting it or making it a trait object.
#[allow(clippy::type_complexity)]
pub fn diff_properties<'a>(
    old_instance: &'a Instance,
    new_instance: &'a Instance,
    filters: &'a BTreeMap<String, PropertyFilter>,
) -> FilterMap<
    Unique<
        Map<
            Chain<
                Box<dyn Iterator<Item = (&'a str, &'a Variant)> + 'a>,
                Box<dyn Iterator<Item = (&'a str, &'a Variant)> + 'a>,
            >,
            impl FnMut((&'a str, &'a Variant)) -> &'a str + 'a,
        >,
    >,
    impl FnMut(&'a str) -> Option<String> + 'a,
> {
    old_instance
        .properties_filtered(filters, false)
        .chain(new_instance.properties_filtered(filters, false))
        .map(|(k, _)| k)
        .unique()
        .filter_map(|key| {
            let new_prop = new_instance
                .properties
                .get(key)
                .or_else(|| get_default_property(&new_instance.class, key));
            let old_prop = old_instance
                .properties
                .get(key)
                .or_else(|| get_default_property(&old_instance.class, key));

            if let (Some(new_prop), Some(old_prop)) = (new_prop, old_prop) {
                if are_variants_similar(old_prop, new_prop) {
                    return None;
                }
            }

            Some(key.to_owned())
        })
}

pub fn diff_individual_property<'a>(
    old_instance: &'a Instance,
    new_instance: &'a Instance,
    key: &'a str,
    filters: &'a BTreeMap<String, PropertyFilter>,
) -> bool {
    let old_value = old_instance
        .properties
        .get(key)
        .filter(|v| filter(&old_instance.class, filters, true)(&(key, *v)));
    let new_value = new_instance
        .properties
        .get(key)
        .filter(|v| filter(&new_instance.class, filters, true)(&(key, *v)));
    old_value != new_value
}

pub fn are_properties_different(
    old_instance: &Instance,
    new_instance: &Instance,
    filters: &BTreeMap<String, PropertyFilter>,
) -> bool {
    diff_properties(old_instance, new_instance, filters).any(|_| true)
}

pub fn are_vector3s_similar(old_vector: &Vector3, new_vector: &Vector3) -> bool {
    approx_eq!(f32, old_vector.x, new_vector.x)
        && approx_eq!(f32, old_vector.y, new_vector.y)
        && approx_eq!(f32, old_vector.z, new_vector.z)
}

pub fn are_vector2s_similar(old_vector: &Vector2, new_vector: &Vector2) -> bool {
    approx_eq!(f32, old_vector.x, new_vector.x) && approx_eq!(f32, old_vector.y, new_vector.y)
}

pub fn are_variants_similar(old_variant: &Variant, new_variant: &Variant) -> bool {
    match (old_variant, new_variant) {
        (Variant::CFrame(old_cframe), Variant::CFrame(new_cframe)) => {
            are_vector3s_similar(&old_cframe.position, &new_cframe.position)
                && are_vector3s_similar(&old_cframe.orientation.x, &new_cframe.orientation.x)
                && are_vector3s_similar(&old_cframe.orientation.y, &new_cframe.orientation.y)
                && are_vector3s_similar(&old_cframe.orientation.z, &new_cframe.orientation.z)
        }
        (Variant::OptionalCFrame(old_cframe), Variant::OptionalCFrame(new_cframe)) => {
            match (old_cframe, new_cframe) {
                (Some(old_cframe), Some(new_cframe)) => are_variants_similar(
                    &Variant::CFrame(*old_cframe),
                    &Variant::CFrame(*new_cframe),
                ),
                _ => false,
            }
        }
        (Variant::Vector3(old_vector), Variant::Vector3(new_vector)) => {
            are_vector3s_similar(old_vector, new_vector)
        }
        (Variant::Vector2(old_vector), Variant::Vector2(new_vector)) => {
            are_vector2s_similar(old_vector, new_vector)
        }
        (Variant::Color3(old_color), Variant::Color3(new_color)) => {
            approx_eq!(f32, old_color.r, new_color.r)
                && approx_eq!(f32, old_color.g, new_color.g)
                && approx_eq!(f32, old_color.b, new_color.b)
        }
        (Variant::Float32(old_float), Variant::Float32(new_float)) => {
            approx_eq!(f32, *old_float, *new_float)
        }
        (Variant::Float64(old_float), Variant::Float64(new_float)) => {
            approx_eq!(f64, *old_float, *new_float)
        }
        (Variant::Attributes(old_attrs), Variant::Attributes(new_attrs)) => {
            for (key, old_value) in old_attrs.iter() {
                if new_attrs.get(key.as_str()).is_none() {
                    return false;
                } else {
                    let new_value = new_attrs.get(key.as_str()).unwrap();

                    if let (Some(old_bytes), Some(new_bytes)) = (
                        old_value.to_variant_binary_string(),
                        new_value.to_variant_binary_string(),
                    ) {
                        if old_bytes != new_bytes {
                            return false;
                        }
                    } else if old_value != new_value {
                        return false;
                    }
                }
            }

            for (key, _) in new_attrs.iter() {
                if old_attrs.get(key.as_str()).is_none() {
                    return false;
                }
            }

            true
        }
        _ => old_variant == new_variant,
    }
}

pub fn display_variant_short(value: &Variant) -> String {
    match value {
        Variant::Axes(v) => format!("{:?}", v),
        Variant::BinaryString(v) => format!(
            "BinaryString(length = {})",
            <rbx_dom_weak::types::BinaryString as AsRef<[u8]>>::as_ref(v).len()
        ),
        Variant::Bool(v) => format!("{}", v),
        Variant::BrickColor(v) => format!("BrickColor({})", v),
        Variant::CFrame(_v) => "CFrame".to_string(),
        Variant::Color3(v) => format!("Color3({}, {}, {})", v.r, v.g, v.b),
        Variant::Color3uint8(v) => format!("Color3uint8({}, {}, {})", v.r, v.g, v.b),
        Variant::ColorSequence(v) => format!(
            "ColorSequence[{}]",
            v.keypoints
                .iter()
                .map(|point| {
                    format!(
                        "({}s, ({}, {}, {}))",
                        point.time, point.color.r, point.color.g, point.color.b
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Variant::Content(v) => format!(
            "Content({})",
            <rbx_dom_weak::types::Content as AsRef<str>>::as_ref(v)
        ),
        Variant::Enum(v) => format!("Enum({})", v.to_u32()),
        Variant::Faces(v) => format!("{:?}", v),
        Variant::Float32(v) => format!("{}", v),
        Variant::Float64(v) => format!("{}", v),
        Variant::Int32(v) => format!("{}", v),
        Variant::Int64(v) => format!("{}", v),
        Variant::NumberRange(v) => format!("NumberRange({}, {})", v.min, v.max),
        Variant::NumberSequence(v) => format!(
            "NumberSequence[{}]",
            v.keypoints
                .iter()
                .map(|point| { format!("({}s, {}, {})", point.time, point.value, point.envelope) })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Variant::PhysicalProperties(v) => format!("{:?}", v),
        Variant::Ray(v) => format!(
            "Ray(({}, {}, {}) -> ({}, {}, {}))",
            v.origin.x, v.origin.y, v.origin.z, v.direction.x, v.direction.y, v.direction.z
        ),
        Variant::Rect(v) => format!(
            "Rect([{}, {}], [{}, {}])",
            v.min.x, v.min.y, v.max.x, v.max.y
        ),
        Variant::Ref(v) => format!("Ref({})", v),
        Variant::Region3(v) => format!(
            "Region3([{}, {}, {}], [{}, {}, {}])",
            v.min.x, v.min.y, v.min.z, v.max.x, v.max.y, v.max.z
        ),
        Variant::Region3int16(v) => format!(
            "Region3int16([{}, {}, {}], [{}, {}, {}])",
            v.min.x, v.min.y, v.min.z, v.max.x, v.max.y, v.max.z
        ),
        Variant::SharedString(v) => format!("SharedString(length = {})", v.data().len()),
        Variant::String(v) => {
            if v.len() < 80 {
                format!("\"{}\"", v)
            } else {
                format!("\"{}...\"", &v[0..77])
            }
        }
        Variant::UDim(v) => format!("UDim({}, {})", v.scale, v.offset),
        Variant::UDim2(v) => format!(
            "UDim2({}, {}, {}, {})",
            v.x.scale, v.x.offset, v.y.scale, v.y.offset
        ),
        Variant::Vector2(v) => format!("Vector2({}, {})", v.x, v.y),
        Variant::Vector2int16(v) => format!("Vector2int16({}, {})", v.x, v.y),
        Variant::Vector3(v) => format!("Vector3({}, {}, {})", v.x, v.y, v.z),
        Variant::Vector3int16(v) => format!("Vector3int16({}, {}, {})", v.x, v.y, v.z),
        Variant::OptionalCFrame(v) => {
            if let Some(_cframe) = v {
                "OptionalCFrame(CFrame)".to_owned()
            } else {
                "OptionalCFrame(None)".to_owned()
            }
        }
        Variant::Tags(v) => format!("Tags[{}]", v.iter().collect::<Vec<_>>().join(", ")),
        Variant::Attributes(v) => format!("Attributes(count = {})", v.iter().count()),
        Variant::Font(v) => format!("{:?}", v),
        Variant::UniqueId(v) => format!("{}", v),
        _ => "Unknown Type".to_owned(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct DeepDiff {
    /// Refs in the old tree that have any changes to their descendants.
    ///
    /// Maps to the count of changed children.
    pub changed_children: HashMap<Ref, u64>,

    /// Refs in the old tree that were removed
    pub removed: HashSet<Ref>,
    /// Refs in the new tree that were added
    pub added: HashSet<Ref>,
    /// Mapping of old-Ref to new-Ref for changed Refs
    pub changed: HashMap<Ref, Ref>,
    /// Mapping of old-Ref to new-Ref for unchanged Refs
    pub unchanged: HashMap<Ref, Ref>,

    /// Mapping of new-Ref to old-Ref for all Refs
    pub new_to_old: HashMap<Ref, Ref>,

    /// Set of refs referenced by any property
    pub property_refs: HashMap<Ref, bool>,
}

pub struct DeepDiffDisplay<'a> {
    diff: &'a DeepDiff,
    old_tree: &'a WeakDom,
    new_tree: &'a WeakDom,
}

impl<'a> DeepDiffDisplay<'a> {
    fn old_name(&self, old_ref: &Ref) -> &str {
        self.old_tree
            .get_by_ref(*old_ref)
            .map_or("[invalid ref]", |i| i.name.as_str())
    }
    fn new_name(&self, new_ref: &Ref) -> &str {
        self.new_tree
            .get_by_ref(*new_ref)
            .map_or("[invalid ref]", |i| i.name.as_str())
    }
}

impl<'a> Display for DeepDiffDisplay<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "DeepDiff")?;
        writeln!(f, "  changed_children")?;
        for (old_ref, count) in &self.diff.changed_children {
            writeln!(f, "    {}  ({})", self.old_name(old_ref), *count)?;
        }
        writeln!(f, "  removed")?;
        for old_ref in &self.diff.removed {
            writeln!(f, "    {}", self.old_name(old_ref))?;
        }
        writeln!(f, "  added")?;
        for new_ref in &self.diff.added {
            writeln!(f, "    {}", self.new_name(new_ref))?;
        }
        writeln!(f, "  changed")?;
        for (old_ref, new_ref) in &self.diff.changed {
            writeln!(
                f,
                "    {} -> {}",
                self.old_name(old_ref),
                self.new_name(new_ref)
            )?;
        }
        writeln!(f, "  unchanged")?;
        for (old_ref, new_ref) in &self.diff.unchanged {
            writeln!(
                f,
                "    {} -> {}",
                self.old_name(old_ref),
                self.new_name(new_ref)
            )?;
        }
        Ok(())
    }
}

pub type ChildrenLists = (HashSet<Ref>, HashSet<Ref>, HashSet<Ref>, HashSet<Ref>);

impl DeepDiff {
    pub fn new<'a>(
        old_tree: &'a WeakDom,
        old_root: Ref,
        new_tree: &mut WeakDom,
        new_root: Ref,
        get_filters: impl Fn(Ref) -> &'a BTreeMap<String, PropertyFilter>,
    ) -> Self {
        let mut diff = Self::default();
        diff.deep_diff(old_tree, old_root, new_tree, new_root, get_filters);
        diff.deduplicate_refs(old_tree, new_tree);
        diff.prune_matching_ref_properties(old_tree, new_tree);
        diff.find_ref_properties(old_tree, new_tree);
        diff
    }

    pub fn display<'a>(
        &'a self,
        old_tree: &'a WeakDom,
        new_tree: &'a WeakDom,
    ) -> DeepDiffDisplay<'a> {
        DeepDiffDisplay {
            diff: self,
            old_tree,
            new_tree,
        }
    }

    pub fn has_changed_descendants(&self, old_ref: Ref) -> bool {
        self.changed_children.contains_key(&old_ref)
    }

    pub fn has_changed_properties(&self, old_ref: Ref) -> bool {
        self.changed.contains_key(&old_ref)
    }
    pub fn is_property_changed(
        &self,
        old_tree: &WeakDom,
        new_tree: &WeakDom,
        old_ref: Ref,
        property_name: &str,
        filters: &BTreeMap<String, PropertyFilter>,
    ) -> bool {
        match self.changed.get(&old_ref) {
            Some(new_ref) => {
                let old_inst = old_tree.get_by_ref(old_ref);
                let new_inst = new_tree.get_by_ref(*new_ref);
                if let (Some(old_inst), Some(new_inst)) = (old_inst, new_inst) {
                    diff_individual_property(old_inst, new_inst, property_name, filters)
                } else {
                    false
                }
            }
            None => false,
        }
    }

    pub fn get_matching_new_ref(&self, old_ref: Ref) -> Option<Ref> {
        self.changed
            .get(&old_ref)
            .or_else(|| self.unchanged.get(&old_ref))
            .cloned()
    }
    pub fn get_matching_old_ref(&self, new_ref: Ref) -> Option<Ref> {
        self.new_to_old.get(&new_ref).cloned()
    }
    pub fn was_removed(&self, old_ref: Ref) -> bool {
        self.removed.contains(&old_ref)
    }
    pub fn was_added(&self, new_ref: Ref) -> bool {
        self.added.contains(&new_ref)
    }

    pub fn is_ref_used_in_property(&self, referent: Ref) -> bool {
        self.property_refs.contains_key(&referent)
    }

    pub fn get_children(
        &self,
        old_tree: &WeakDom,
        new_tree: &WeakDom,
        old_ref: Ref,
    ) -> Option<ChildrenLists> {
        let new_ref = match self.get_matching_new_ref(old_ref) {
            Some(new_ref) => new_ref,
            None => return None,
        };

        let mut added = HashSet::new();
        let mut removed = HashSet::new();
        let mut changed = HashSet::new();
        let mut unchanged = HashSet::new();

        let old_inst = old_tree.get_by_ref(old_ref)?;
        let new_inst = new_tree.get_by_ref(new_ref)?;

        for child_ref in old_inst.children() {
            if self.was_removed(*child_ref) {
                removed.insert(*child_ref);
            } else if self.has_changed_properties(*child_ref)
                || self.has_changed_descendants(*child_ref)
            {
                changed.insert(*child_ref);
            } else {
                unchanged.insert(*child_ref);
            }
        }
        for child_ref in new_inst.children() {
            if self.was_added(*child_ref) {
                added.insert(*child_ref);
            }
        }

        Some((added, removed, changed, unchanged))
    }

    pub fn show_diff(&self, old_tree: &WeakDom, new_tree: &WeakDom, path: &[String]) {
        let old_root_ref = 'old_ref: {
            let mut item = old_tree.root_ref();
            for name in path.iter() {
                let next_item = old_tree
                    .get_by_ref(item)
                    .unwrap_or_else(|| {
                        panic!("Invalid ref {} when traversing tree (next: {})", item, name)
                    })
                    .children()
                    .iter()
                    .find(|&child_ref| old_tree.get_by_ref(*child_ref).unwrap().name == *name);
                item = match next_item {
                    Some(item) => *item,
                    None => {
                        break 'old_ref None;
                    }
                };
            }
            Some(item)
        };
        let new_root_ref = 'new_ref: {
            let mut item = new_tree.root_ref();
            for name in path.iter() {
                let next_item = new_tree
                    .get_by_ref(item)
                    .unwrap_or_else(|| {
                        panic!("Invalid ref {} when traversing tree (next: {})", item, name)
                    })
                    .children()
                    .iter()
                    .find(|&child_ref| new_tree.get_by_ref(*child_ref).unwrap().name == *name);
                item = match next_item {
                    Some(item) => *item,
                    None => {
                        break 'new_ref None;
                    }
                };
            }
            Some(item)
        };

        let get_name_old = |old_ref: Ref| -> &str {
            old_tree
                .get_by_ref(old_ref)
                .map_or("[invalid ref]", |v| v.name.as_str())
        };
        let get_name_new = |new_ref: Ref| -> &str {
            new_tree
                .get_by_ref(new_ref)
                .map_or("[invalid ref]", |v| v.name.as_str())
        };

        match (old_root_ref, new_root_ref) {
            (None, None) => {
                println!("Could not find {} in the old or new tree", path.join("."));
            }
            (Some(old_ref), None) => {
                println!("- {}", get_name_old(old_ref));
            }
            (None, Some(new_ref)) => {
                println!("+ {}", get_name_new(new_ref));
            }
            (Some(old_root_ref), Some(_)) => {
                let mut processing = vec![(old_root_ref, 0)];
                while let Some((old_ref, tabs)) = processing.pop() {
                    let new_ref = self.get_matching_new_ref(old_ref);
                    match new_ref {
                        None => {
                            println!("{}- {}", "  ".repeat(tabs), get_name_old(old_ref));
                        }
                        Some(new_ref) => {
                            if self.has_changed_properties(old_ref) {
                                println!("{}~ {}", "  ".repeat(tabs), get_name_old(old_ref));
                            } else if self.has_changed_descendants(old_ref) {
                                println!(
                                    "{}  {}  (~{} children)",
                                    "  ".repeat(tabs),
                                    get_name_old(old_ref),
                                    self.changed_children[&old_ref]
                                );
                            }
                            if self.has_changed_properties(old_ref) {
                                let old_inst = old_tree.get_by_ref(old_ref).unwrap();
                                let new_inst = new_tree.get_by_ref(new_ref).unwrap();

                                if let Some(true) = self.property_refs.get(&old_ref) {
                                    println!(
                                        "{}~ referred to by a changed property",
                                        "  ".repeat(tabs + 2)
                                    )
                                }

                                let changed_properties =
                                    diff_properties(old_inst, new_inst, default_filters_diff());
                                for property_name in changed_properties {
                                    let old_value = old_inst.properties.get(&property_name);
                                    let new_value = new_inst.properties.get(&property_name);
                                    match (old_value, new_value) {
                                        (None, None) => (),
                                        (Some(old_value), Some(new_value)) => {
                                            println!(
                                                "{}~ {}: {} -> {}",
                                                "  ".repeat(tabs + 2),
                                                property_name,
                                                display_variant_short(old_value),
                                                display_variant_short(new_value)
                                            );
                                        }
                                        (Some(old_value), None) => {
                                            println!(
                                                "{}~ {}: {} -> None",
                                                "  ".repeat(tabs + 2),
                                                property_name,
                                                display_variant_short(old_value)
                                            )
                                        }
                                        (None, Some(new_value)) => {
                                            println!(
                                                "{}~ {}: None -> {}",
                                                "  ".repeat(tabs + 2),
                                                property_name,
                                                display_variant_short(new_value)
                                            )
                                        }
                                    }
                                }
                            }
                            if self.has_changed_descendants(old_ref) {
                                let changes = self.get_children(old_tree, new_tree, old_ref);
                                if let Some((added, removed, changed, _unchanged)) = changes {
                                    for removed_ref in removed {
                                        println!(
                                            "{}- {}",
                                            "  ".repeat(tabs + 1),
                                            get_name_old(removed_ref)
                                        );
                                    }
                                    for added_ref in added {
                                        println!(
                                            "{}+ {}",
                                            "  ".repeat(tabs + 1),
                                            get_name_new(added_ref)
                                        );
                                    }
                                    for changed_ref in changed {
                                        processing.push((changed_ref, tabs + 1));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn prune_matching_ref_properties(&mut self, old_tree: &WeakDom, new_tree: &WeakDom) {
        let changed: Vec<(Ref, Ref)> = self.changed.iter().map(|(v1, v2)| (*v1, *v2)).collect();
        for (old_ref, new_ref) in changed.into_iter() {
            let old_inst = old_tree.get_by_ref(old_ref).unwrap();
            let new_inst = new_tree.get_by_ref(new_ref).unwrap();
            let only_unchanged_ref_props =
                diff_properties(old_inst, new_inst, default_filters_diff()).all(|property_name| {
                    let old_prop = old_inst.properties.get(&property_name);
                    let new_prop = new_inst.properties.get(&property_name);
                    if let Some(Variant::Ref(old_ref)) = old_prop {
                        if let Some(Variant::Ref(new_ref)) = new_prop {
                            if self.new_to_old.get(new_ref) == Some(old_ref) {
                                return true;
                            }
                        }
                    }
                    false
                });
            if only_unchanged_ref_props {
                self.changed.remove(&old_ref);
                self.new_to_old.remove(&new_ref);
                self.unchanged.insert(old_ref, new_ref);
                if !self.has_changed_descendants(old_ref) {
                    self.unmark_ancestors(old_tree, old_inst.parent())
                }
            }
        }
    }

    fn move_ref(&mut self, new_dom_ref: Ref, replacement: Ref) {
        if let Some(old_ref) = self.new_to_old.remove(&new_dom_ref) {
            self.new_to_old.insert(replacement, old_ref);

            if self.changed.remove(&old_ref).is_some() {
                self.changed.insert(old_ref, replacement);
            } else if self.unchanged.remove(&old_ref).is_some() {
                self.unchanged.insert(old_ref, replacement);
            }
        }

        if let Some(v) = self.property_refs.remove(&new_dom_ref) {
            self.property_refs.insert(replacement, v);
        }
    }

    fn deduplicate_refs(&mut self, old_tree: &WeakDom, new_tree: &mut WeakDom) {
        let mut ref_map = BTreeMap::new();
        {
            // Fix duplicate refs
            for referent in new_tree.descendants().collect::<Vec<_>>() {
                let matching_old_ref = self.get_matching_old_ref(referent);
                let replacement = match matching_old_ref {
                    Some(old_ref) => {
                        if referent != old_ref {
                            Some(old_ref)
                        } else {
                            None
                        }
                    }
                    None => {
                        if old_tree.get_by_ref(referent).is_some() {
                            Some(loop {
                                let replacement = Ref::new();
                                if old_tree.get_by_ref(replacement).is_none() {
                                    break replacement;
                                }
                            })
                        } else {
                            None
                        }
                    }
                };
                if let Some(replacement) = replacement {
                    new_tree.swap_ref(referent, replacement);
                    self.move_ref(referent, replacement);
                    ref_map.insert(referent, replacement);
                }
            }
        }
        {
            // Fix properties
            for referent in new_tree.descendants().collect::<Vec<_>>() {
                for (_k, v) in new_tree
                    .get_by_ref_mut(referent)
                    .unwrap()
                    .properties
                    .iter_mut()
                {
                    if let Variant::Ref(prop_ref) = v {
                        if let Some(fixed_ref) = ref_map.get(prop_ref) {
                            *v = Variant::Ref(*fixed_ref);
                        }
                    }
                }
            }
        }
    }

    fn find_ref_properties(&mut self, old_tree: &WeakDom, new_tree: &WeakDom) {
        for old_ref in old_tree.descendants() {
            for (property_name, v) in old_tree.get_by_ref(old_ref).unwrap().properties.iter() {
                if let Variant::Ref(prop_ref) = v {
                    self.property_refs.insert(
                        *prop_ref,
                        self.is_property_changed(
                            old_tree,
                            new_tree,
                            old_ref,
                            property_name,
                            &BTreeMap::new(),
                        ),
                    );
                }
            }
        }

        for new_ref in new_tree.descendants() {
            for (property_name, v) in new_tree.get_by_ref(new_ref).unwrap().properties.iter() {
                if let Variant::Ref(prop_ref) = v {
                    let is_changed = if let Some(old_ref) = self.get_matching_old_ref(new_ref) {
                        self.is_property_changed(
                            old_tree,
                            new_tree,
                            old_ref,
                            property_name,
                            &BTreeMap::new(),
                        )
                    } else {
                        true
                    };

                    self.property_refs.insert(*prop_ref, is_changed);
                }
            }
        }

        // Mark all instances that are referred to by a changed property as
        // changed instances as well, to ensure that they're re-serialized with
        // a persistent ref id if they weren't already.
        for old_ref in self
            .property_refs
            .iter()
            .filter(|(_prop_ref, &is_changed)| is_changed)
            .map(|(&prop_ref, _)| prop_ref)
            .collect_vec()
        {
            if let Some(old_inst) = old_tree.get_by_ref(old_ref) {
                if self.unchanged.contains_key(&old_ref) {
                    if let Some(new_ref) = self.get_matching_new_ref(old_ref) {
                        self.changed.insert(old_ref, new_ref);
                        self.unchanged.remove(&old_ref);
                        self.mark_ancestors(old_tree, old_inst.parent());
                    }
                }
            }
        }
    }

    fn match_children(
        &mut self,
        old_tree: &WeakDom,
        old_instance: &Instance,
        new_tree: &WeakDom,
        new_instance: &Instance,
        filters: &BTreeMap<String, PropertyFilter>,
    ) -> HashMap<Ref, Ref> {
        let mut matches = HashMap::new();

        let mut any_changes = false;

        let mut by_name: HashMap<&str, (HashSet<Ref>, HashSet<Ref>)> = HashMap::new();
        for child_ref in old_instance.children() {
            let child = old_tree.get_by_ref(*child_ref).unwrap();
            by_name.entry(&child.name).or_default().0.insert(*child_ref);
        }

        for child_ref in new_instance.children() {
            let child = new_tree.get_by_ref(*child_ref).unwrap();
            by_name.entry(&child.name).or_default().1.insert(*child_ref);
        }

        let mut scorer = SimilarityScorer {
            old_tree,
            new_tree,
            filters,
            prop_cache_old: HashMap::new(),
            prop_cache_new: HashMap::new(),
        };

        for (_name, (old_children, mut new_children)) in by_name.into_iter() {
            for old_child_ref in old_children {
                if new_children.is_empty() {
                    self.removed.insert(old_child_ref);
                    any_changes = true;
                    continue;
                }

                if new_children.contains(&old_child_ref) {
                    matches.insert(old_child_ref, old_child_ref);
                    new_children.remove(&old_child_ref);
                    continue;
                }

                // this is thorough but slow. we should set up a fast, not-thorough
                // solution for big trees.
                let old_child = old_tree.get_by_ref(old_child_ref).unwrap();
                let best_match_iter = new_children.iter().map(|new_child_ref| {
                    let new_child = new_tree.get_by_ref(*new_child_ref).unwrap();
                    let (diff_score, same_score) = scorer.similarity_score(old_child, new_child);

                    let percent_same_score = (same_score * 100) / (diff_score + same_score);

                    (*new_child_ref, percent_same_score)
                });

                let mut best_match = None;
                for (new_child_ref, percent_same_score) in best_match_iter {
                    if percent_same_score == 100 {
                        best_match = Some((new_child_ref, percent_same_score));
                        break;
                    } else if percent_same_score > best_match.map_or(-1, |x| x.1) {
                        best_match = Some((new_child_ref, percent_same_score));
                    }
                }

                if let Some((new_child_ref, _percent_same_score)) = best_match {
                    matches.insert(old_child_ref, new_child_ref);
                    new_children.remove(&new_child_ref);
                } else {
                    self.removed.insert(old_child_ref);
                    any_changes = true;
                }
            }

            for new_child_ref in new_children {
                self.added.insert(new_child_ref);
                any_changes = true;
            }
        }

        if any_changes {
            self.mark_ancestors(old_tree, old_instance.referent());
        }

        matches
    }

    fn mark_ancestors(&mut self, old_tree: &WeakDom, parent_ref: Ref) {
        // log::trace!("marking ancestors for {}", ancestor_ref);
        let mut ancestor_ref = parent_ref;
        while let Some(old_inst) = old_tree.get_by_ref(ancestor_ref) {
            if self.changed_children.contains_key(&ancestor_ref) {
                self.changed_children
                    .entry(ancestor_ref)
                    .and_modify(|v| *v += 1);
                break;
            }

            self.changed_children.insert(ancestor_ref, 1);

            if self.changed.contains_key(&ancestor_ref) {
                break;
            }
            ancestor_ref = old_inst.parent();
        }
    }

    fn unmark_ancestors(&mut self, old_tree: &WeakDom, parent_ref: Ref) {
        // log::trace!("unmarking ancestors for {}", ancestor_ref);
        let mut ancestor_ref = parent_ref;
        while let Some(old_inst) = old_tree.get_by_ref(ancestor_ref) {
            if !self.changed_children.contains_key(&ancestor_ref) {
                break;
            }

            let changed_children = self.changed_children[&ancestor_ref] - 1;
            if changed_children > 0 {
                self.changed_children.insert(ancestor_ref, changed_children);
                break;
            }

            self.changed_children.remove(&ancestor_ref);
            ancestor_ref = old_inst.parent();
        }
    }

    fn deep_diff<'a>(
        &mut self,
        old_tree: &'a WeakDom,
        old_root: Ref,
        new_tree: &WeakDom,
        new_root: Ref,
        get_filters: impl Fn(Ref) -> &'a BTreeMap<String, PropertyFilter>,
    ) {
        let mut process: Vec<(Ref, Ref)> = vec![(old_root, new_root)];

        while let Some((old_ref, new_ref)) = process.pop() {
            let old_inst = old_tree.get_by_ref(old_ref).unwrap();
            let new_inst = new_tree.get_by_ref(new_ref).unwrap();

            let filters = get_filters(old_ref);

            if are_properties_different(old_inst, new_inst, filters) {
                self.changed.insert(old_ref, new_ref);
                self.new_to_old.insert(new_ref, old_ref);
                self.mark_ancestors(old_tree, old_inst.parent());
            } else {
                self.unchanged.insert(old_ref, new_ref);
                self.new_to_old.insert(new_ref, old_ref);
            }

            if !old_inst.children().is_empty() || !new_inst.children().is_empty() {
                let matches = self.match_children(old_tree, old_inst, new_tree, new_inst, filters);
                process.extend(matches.into_iter());
            }
        }
    }
}

pub struct SimilarityScorer<'a> {
    old_tree: &'a WeakDom,
    new_tree: &'a WeakDom,
    filters: &'a BTreeMap<String, PropertyFilter>,
    prop_cache_old: HashMap<Ref, BTreeMap<&'a str, &'a Variant>>,
    prop_cache_new: HashMap<Ref, BTreeMap<&'a str, &'a Variant>>,
}

impl<'a> SimilarityScorer<'a> {
    pub fn similarity_score(
        &mut self,
        old_instance: &'a Instance,
        new_instance: &'a Instance,
    ) -> (i32, i32) {
        let mut diff_score = 0;
        let mut same_score = 0;

        let old_properties = self
            .prop_cache_old
            .entry(old_instance.referent())
            .or_insert_with(|| old_instance.properties_filtered_map(self.filters, false));
        let new_properties = self
            .prop_cache_new
            .entry(new_instance.referent())
            .or_insert_with(|| new_instance.properties_filtered_map(self.filters, false));

        for (k, old_property) in old_properties.iter() {
            if let Some(new_property) = new_properties.get(k) {
                if are_variants_similar(old_property, new_property) {
                    same_score += 1;
                    continue;
                }
            }
            diff_score += 1;
        }
        for (k, _v) in new_properties.iter() {
            if !old_properties.contains_key(k) {
                diff_score += 1;
            }
        }

        if old_instance.class == new_instance.class {
            same_score += 1;
        } else {
            diff_score += 1;
        }

        if old_instance.name == new_instance.name {
            same_score += 1;
        } else {
            diff_score += 1;
        }

        let mut children_by_name: HashMap<&str, i32> = HashMap::new();
        for child_ref in old_instance.children() {
            let child = self.old_tree.get_by_ref(*child_ref).unwrap();
            diff_score += 1;
            children_by_name
                .entry(&child.name)
                .or_default()
                .add_assign(1);
        }
        for child_ref in new_instance.children() {
            let child = self.new_tree.get_by_ref(*child_ref).unwrap();
            diff_score += 1;
            children_by_name.entry(&child.name).and_modify(|v| {
                if v > &mut 0 {
                    same_score += 1;
                    diff_score -= 2; // subtract diff added by both loops
                    *v -= 1;
                }
            });
        }

        (diff_score, same_score)
    }
}

use std::{
    borrow::Cow,
    collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Display,
    hash::{Hash, Hasher},
    iter::{Chain, FilterMap, Map},
};

use float_cmp::approx_eq;
use itertools::{Itertools, Unique};
use rbx_dom_weak::{
    types::{Attributes, BinaryString, Color3, Ref, Variant, Vector2, Vector3},
    Instance, WeakDom,
};

use super::{
    default_filters_diff, filter, get_default_property, InstanceExtra, PropertiesFiltered,
    PropertyFilter, ToVariantBinaryString, WeakDomExtra,
};

const ROJO_DEDUP_KEY: &str = "__RojoDeduplicate";

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
                (None, None) => true,
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

fn round_places_f64(v: f64, places: i32) -> f64 {
    (v * 10f64.powi(places)).round() / 10f64.powi(places)
}

fn round_places_f32(v: f32, places: i32) -> f32 {
    (v * 10f32.powi(places)).round() / 10f32.powi(places)
}

pub fn round_variant_floats_for_hash(v: &Variant) -> Cow<Variant> {
    // limited coverage for now, will expand if it's an issue!
    match v {
        Variant::Float32(v) => Cow::Owned(Variant::Float32(round_places_f32(*v, 3))),
        Variant::Float64(v) => Cow::Owned(Variant::Float64(round_places_f64(*v, 3))),
        Variant::Vector2(v) => Cow::Owned(Variant::Vector2(Vector2 {
            x: round_places_f32(v.x, 3),
            y: round_places_f32(v.y, 3),
        })),
        Variant::Vector3(v) => Cow::Owned(Variant::Vector3(Vector3 {
            x: round_places_f32(v.x, 3),
            y: round_places_f32(v.y, 3),
            z: round_places_f32(v.z, 3),
        })),
        Variant::Color3(v) => Cow::Owned(Variant::Color3(Color3 {
            r: round_places_f32(v.r, 3),
            g: round_places_f32(v.g, 3),
            b: round_places_f32(v.b, 3),
        })),
        _ => Cow::Borrowed(v),
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
        Variant::Float32(v) => format!("{} (f32)", v),
        Variant::Float64(v) => format!("{} (f64)", v),
        Variant::Int32(v) => format!("{} (i32)", v),
        Variant::Int64(v) => format!("{} (i64)", v),
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
        Variant::String(v) => if v.len() < 50 {
            format!("\"{}\"", v)
        } else {
            format!("\"{}...\"", &v[0..47])
        }
        .replace('\n', "\\n")
        .replace('\t', "\\t"),
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
        get_property_filters: impl Fn(Ref) -> &'a BTreeMap<String, PropertyFilter>,
        should_skip: impl Fn(Ref) -> bool,
    ) -> Self {
        let mut diff = Self::default();

        diff.update_rojo_dedup_ids(new_tree);

        diff.deep_diff(
            old_tree,
            old_root,
            new_tree,
            new_root,
            &get_property_filters,
            &should_skip,
        );
        let _ref_map = diff.deduplicate_refs(old_tree, new_tree);

        // rescan after fixing properties as it allows us to re-evaluate ref
        // properties with correct values. this is slow so we should probably
        // find a faster solution someday.
        // let new_root = *ref_map.get(&new_root).unwrap_or(&new_root);
        // diff.clear();
        // log::info!("Beginning second diff pass");
        // diff.deep_diff(
        //     old_tree,
        //     old_root,
        //     new_tree,
        //     new_root,
        //     &get_property_filters,
        //     &should_skip,
        // );

        diff.prune_matching_ref_properties(old_tree, new_tree);
        diff.find_ref_properties(old_tree, new_tree);
        diff
    }

    pub fn clear(&mut self) {
        self.changed_children.clear();
        self.removed.clear();
        self.added.clear();
        self.changed.clear();
        self.unchanged.clear();
        self.new_to_old.clear();
        self.property_refs.clear();
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

    pub fn show_diff<'a>(
        &self,
        old_tree: &WeakDom,
        new_tree: &WeakDom,
        path: &[String],
        get_filters: impl Fn(Ref) -> &'a BTreeMap<String, PropertyFilter>,
        should_skip: impl Fn(Ref) -> bool,
    ) -> bool {
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

        let mut any_changes = false;
        match (old_root_ref, new_root_ref) {
            (None, None) => {
                println!("Could not find {} in the old or new tree", path.join("."));
            }
            (Some(old_ref), None) => {
                println!("- {}", get_name_old(old_ref));
                any_changes = true;
            }
            (None, Some(new_ref)) => {
                println!("+ {}", get_name_new(new_ref));
                any_changes = true;
            }
            (Some(old_root_ref), Some(_)) => {
                let mut processing = vec![(old_root_ref, 0)];
                while let Some((old_ref, tabs)) = processing.pop() {
                    let new_ref = self.get_matching_new_ref(old_ref);
                    match new_ref {
                        None => {
                            println!("{}- {}", "  ".repeat(tabs), get_name_old(old_ref));
                            any_changes = true;
                        }
                        Some(new_ref) => {
                            let old_inst = old_tree.get_by_ref(old_ref).unwrap();
                            let new_inst = new_tree.get_by_ref(new_ref).unwrap();

                            if should_skip(old_ref) {
                                continue;
                            }

                            let filters = get_filters(old_ref);

                            let should_print = (self.has_changed_properties(old_ref)
                                && diff_properties(old_inst, new_inst, filters).any(|_| true))
                                || self.has_changed_descendants(old_ref);

                            if !should_print {
                                continue;
                            }

                            let mut prefix = "  ";
                            let mut postfix = "".to_string();

                            if self.has_changed_properties(old_ref) {
                                prefix = "~ ";
                            }
                            if self.has_changed_descendants(old_ref) {
                                postfix =
                                    format!("  (~{} children)", self.changed_children[&old_ref]);
                            }

                            let (diff_score, same_score) =
                                SimilarityScorer::new(old_tree, new_tree, default_filters_diff())
                                    .similarity_score(old_inst, new_inst);
                            let percent_same_score = (same_score * 100) / (diff_score + same_score);

                            println!(
                                "{}{}{}  ({}%  {},{}){}",
                                "  ".repeat(tabs),
                                prefix,
                                get_name_old(old_ref),
                                percent_same_score,
                                same_score,
                                diff_score,
                                postfix
                            );
                            any_changes = true;

                            if self.has_changed_properties(old_ref) {
                                if let Some(true) = self.property_refs.get(&old_ref) {
                                    println!(
                                        "{}~ referred to by a changed property",
                                        "  ".repeat(tabs + 2)
                                    )
                                }

                                let changed_properties =
                                    diff_properties(old_inst, new_inst, default_filters_diff());
                                let mut changed_properties = changed_properties.collect_vec();
                                changed_properties.sort();

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
                                    let mut added =
                                        added.into_iter().map(get_name_new).collect_vec();
                                    added.sort();
                                    let mut removed =
                                        removed.into_iter().map(get_name_old).collect_vec();
                                    removed.sort();
                                    let mut changed = changed
                                        .into_iter()
                                        .map(|v| (get_name_old(v), v))
                                        .collect_vec();
                                    changed.sort_by(|(a, _), (b, _)| a.cmp(b));

                                    for removed_name in removed {
                                        println!("{}- {}", "  ".repeat(tabs + 1), removed_name);
                                    }
                                    for added_name in added {
                                        println!("{}+ {}", "  ".repeat(tabs + 1), added_name);
                                    }
                                    for (_, changed_ref) in changed {
                                        processing.push((changed_ref, tabs + 1));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if !any_changes {
            println!("No changes");
        }

        any_changes
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

    fn deduplicate_refs(
        &mut self,
        old_tree: &WeakDom,
        new_tree: &mut WeakDom,
    ) -> BTreeMap<Ref, Ref> {
        let mut ref_map: BTreeMap<Ref, Ref> = BTreeMap::new();
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
                for (_property_name, v) in new_tree
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
        ref_map
    }

    fn find_ref_properties(&mut self, old_tree: &WeakDom, new_tree: &WeakDom) {
        for old_ref in old_tree.descendants() {
            for (property_name, v) in old_tree.get_by_ref(old_ref).unwrap().properties.iter() {
                if let Variant::Ref(prop_ref) = v {
                    if prop_ref.is_some() {
                        let is_changed = self.is_property_changed(
                            old_tree,
                            new_tree,
                            old_ref,
                            property_name,
                            &BTreeMap::new(),
                        );

                        if is_changed {
                            log::trace!(
                                "marking {} as used in a changed property {} (old tree)",
                                prop_ref,
                                property_name
                            );
                        }

                        self.property_refs.insert(*prop_ref, is_changed);
                    }
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
                        false
                    };

                    if is_changed {
                        log::trace!(
                            "marking {} as used in a changed property {} (new tree)",
                            prop_ref,
                            property_name
                        );
                    }

                    self.property_refs.insert(*prop_ref, is_changed);
                }
            }
        }

        // Mark all instances that are referred to by a changed property as
        // changed instances as well, to ensure that they're re-serialized with
        // a persistent ref id if they weren't already.
        let changed_refs_list = self
            .property_refs
            .iter()
            .filter(|(_prop_ref, &is_changed)| is_changed)
            .map(|(&prop_ref, _)| prop_ref)
            .collect_vec();

        for old_ref in changed_refs_list {
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

        let mut scorer = SimilarityScorer::new(old_tree, new_tree, filters);

        for (_name, (mut old_children, mut new_children)) in by_name.into_iter() {
            while !old_children.is_empty() {
                if new_children.is_empty() {
                    for old_child_ref in old_children {
                        self.removed.insert(old_child_ref);
                        any_changes = true;
                    }
                    break;
                }

                let mut similarity_score_map: BTreeMap<Ref, (Ref, i32)> = BTreeMap::new();

                for &old_child_ref in old_children.iter() {
                    let old_child = old_tree.get_by_ref(old_child_ref).unwrap();

                    let best_match_iter = new_children.iter().map(|new_child_ref| {
                        let new_child = new_tree.get_by_ref(*new_child_ref).unwrap();
                        let (diff_score, same_score) =
                            scorer.similarity_score(old_child, new_child);

                        let percent_same_score = (same_score * 100) / (diff_score + same_score);

                        (*new_child_ref, percent_same_score)
                    });

                    let mut best_match: Option<(Ref, i32)> = None;
                    for (new_child_ref, percent_same_score) in best_match_iter {
                        if best_match.is_none() || percent_same_score > best_match.unwrap().1 {
                            best_match = Some((new_child_ref, percent_same_score));
                            if percent_same_score == 100 {
                                break;
                            }
                        }
                    }

                    similarity_score_map.insert(old_child_ref, best_match.unwrap());
                }

                let mut top_similarity_scores: Vec<_> = similarity_score_map
                    .into_iter()
                    .map(|(k, (v1, v2))| (k, v1, v2))
                    .collect();
                top_similarity_scores.sort_by(|(_, _, score_a), (_, _, score_b)| {
                    score_b.partial_cmp(score_a).unwrap()
                });

                for (old_child_ref, new_child_ref, _score) in top_similarity_scores {
                    if new_children.contains(&new_child_ref) {
                        matches.insert(old_child_ref, new_child_ref);
                        new_children.remove(&new_child_ref);
                        old_children.remove(&old_child_ref);
                    }
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
        get_filters: &impl Fn(Ref) -> &'a BTreeMap<String, PropertyFilter>,
        should_skip: &impl Fn(Ref) -> bool,
    ) {
        let mut process: Vec<(Ref, Ref)> = vec![(old_root, new_root)];

        while let Some((old_ref, new_ref)) = process.pop() {
            if should_skip(old_ref) {
                continue;
            }

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

    fn update_rojo_dedup_ids(&mut self, new_tree: &mut WeakDom) {
        let mut processing: Vec<Ref> = vec![new_tree.root_ref()];

        while let Some(new_ref) = processing.pop() {
            let new_inst = new_tree.get_by_ref(new_ref).unwrap();
            let mut children_by_properties_hashed: BTreeMap<(&str, &str, u64), Vec<Ref>> =
                BTreeMap::new();

            for child_ref in new_inst.children() {
                let child = new_tree.get_by_ref(*child_ref).unwrap();
                let mut properties = child
                    .properties_filtered(default_filters_diff(), true)
                    .collect_vec();
                properties.sort_by(|a, b| a.0.cmp(b.0));

                let properties_hashed = properties
                    .into_iter()
                    .fold(DefaultHasher::new(), |mut hasher, (key, value)| {
                        let value = round_variant_floats_for_hash(value);
                        match value.as_ref() {
                            // Types are whitelisted so that new problematic
                            // types don't interfere.
                            Variant::Axes(_)
                            | Variant::Bool(_)
                            | Variant::BrickColor(_)
                            | Variant::CFrame(_)
                            | Variant::Color3(_)
                            | Variant::ColorSequence(_)
                            | Variant::Content(_)
                            | Variant::Enum(_)
                            | Variant::Faces(_)
                            | Variant::NumberRange(_)
                            | Variant::NumberSequence(_)
                            | Variant::PhysicalProperties(_)
                            | Variant::Ray(_)
                            | Variant::Rect(_)
                            | Variant::UDim(_)
                            | Variant::UDim2(_)
                            | Variant::OptionalCFrame(_)
                            | Variant::Tags(_)
                            | Variant::Font(_)
                            | Variant::MaterialColors(_)
                            | Variant::Region3(_)
                            | Variant::Vector3(_)
                            | Variant::Vector2(_)
                            | Variant::Float64(_) => value.hash(&mut hasher),
                            // Handle "look-alikes"
                            Variant::Color3uint8(v) => {
                                Variant::Color3((*v).into()).hash(&mut hasher)
                            }
                            Variant::Region3int16(v) => {
                                Variant::Region3((*v).into()).hash(&mut hasher)
                            }
                            Variant::Vector3int16(v) => {
                                Variant::Vector3((*v).into()).hash(&mut hasher)
                            }
                            Variant::Vector2int16(v) => {
                                Variant::Vector2((*v).into()).hash(&mut hasher)
                            }
                            Variant::Float32(v) => Variant::Float64(*v as f64).hash(&mut hasher),
                            Variant::Int32(v) => Variant::Float64(*v as f64).hash(&mut hasher),
                            Variant::Int64(v) => Variant::Float64(*v as f64).hash(&mut hasher),
                            // Handle strings; this is not equivalent to hashing
                            // the variant itself, as we're skipping hashing the
                            // variant type.
                            Variant::String(v) => v.as_bytes().hash(&mut hasher),
                            Variant::SharedString(v) => v.as_ref().hash(&mut hasher),
                            Variant::BinaryString(v) => {
                                <BinaryString as AsRef<[u8]>>::as_ref(v).hash(&mut hasher)
                            }
                            // Handle attributes; make sure to remove special
                            // Rojo-only attributes.
                            Variant::Attributes(value) => {
                                if value.get(ROJO_DEDUP_KEY).is_some() {
                                    let mut new_attributes = value.clone();
                                    new_attributes.remove(ROJO_DEDUP_KEY);

                                    // skip empty
                                    if !new_attributes.iter().any(|_| true) {
                                        return hasher;
                                    }

                                    new_attributes.hash(&mut hasher)
                                } else {
                                    // skip empty
                                    if !value.iter().any(|_| true) {
                                        return hasher;
                                    }

                                    value.hash(&mut hasher)
                                }
                            }
                            // Skip variants that are not going to be consistent
                            // between instances.
                            Variant::UniqueId(_) | Variant::Ref(_) | _ => return hasher,
                        }

                        key.hash(&mut hasher);

                        hasher
                    })
                    .finish();

                children_by_properties_hashed
                    .entry((child.class.as_str(), child.name.as_str(), properties_hashed))
                    .or_default()
                    .push(*child_ref);
            }

            // get rid of borrowed names and classes
            let children_by_properties_hashed =
                children_by_properties_hashed.into_values().collect_vec();

            for children in children_by_properties_hashed {
                if children.len() == 1 {
                    let child_ref = children[0];
                    let child = new_tree.get_by_ref_mut(child_ref).unwrap();
                    let attributes = child.properties.get_mut("Attributes");
                    if let Some(Variant::Attributes(attributes)) = attributes {
                        attributes.remove(ROJO_DEDUP_KEY);
                    }
                } else {
                    let mut used_unique_ids = BTreeSet::new();
                    let mut needs_new_unique_id = Vec::new();
                    for child_ref in children {
                        let child = new_tree.get_by_ref(child_ref).unwrap();
                        let attributes = child.get_attributes_opt();
                        if let Some(attributes) = attributes {
                            let unique_id = attributes.get(ROJO_DEDUP_KEY);
                            if let Some(unique_id) = unique_id {
                                let unique_id: Option<&[u8]> = match unique_id {
                                    Variant::String(s) => Some(s.as_ref()),
                                    Variant::BinaryString(s) => Some(s.as_ref()),
                                    Variant::SharedString(s) => Some(s.as_ref()),
                                    _ => {
                                        log::warn!(
                                            "Bad dedup id {:?} for {}",
                                            unique_id,
                                            child.name
                                        );
                                        None
                                    }
                                };
                                if let Some(unique_id) = unique_id {
                                    if used_unique_ids.insert(unique_id) {
                                        continue;
                                    }
                                    log::debug!(
                                        "Duplicated dedup id {} for {}",
                                        std::string::String::from_utf8_lossy(unique_id),
                                        child.name
                                    );
                                }
                            }
                        }
                        log::debug!("Missing dedup id for {}", child.name);
                        needs_new_unique_id.push(child_ref);
                    }

                    for child_ref in needs_new_unique_id {
                        let child = new_tree.get_by_ref_mut(child_ref).unwrap();
                        let attributes = if let Some(Variant::Attributes(attributes)) =
                            child.properties.get_mut("Attributes")
                        {
                            attributes
                        } else {
                            child.properties.insert(
                                "Attributes".to_string(),
                                Variant::Attributes(Attributes::default()),
                            );
                            match child.properties.get_mut("Attributes").unwrap() {
                                Variant::Attributes(attributes) => attributes,
                                _ => unreachable!(),
                            }
                        };

                        let next_id = uuid::Uuid::new_v4().as_simple().to_string();

                        attributes.insert(ROJO_DEDUP_KEY.to_string(), Variant::String(next_id));
                    }
                }
            }

            // avoid a long-lived borrow by grabbing the instance again.
            let new_inst = new_tree.get_by_ref(new_ref).unwrap();
            processing.extend(new_inst.children());
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
    pub fn new(
        old_tree: &'a WeakDom,
        new_tree: &'a WeakDom,
        filters: &'a BTreeMap<String, PropertyFilter>,
    ) -> Self {
        Self {
            old_tree,
            new_tree,
            filters,
            prop_cache_old: HashMap::new(),
            prop_cache_new: HashMap::new(),
        }
    }

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

        let old_attributes = old_instance.get_attributes_opt();
        let new_attributes = new_instance.get_attributes_opt();

        if let (Some(old_attributes), Some(new_attributes)) = (old_attributes, new_attributes) {
            let old_dedup_id = old_attributes.get(ROJO_DEDUP_KEY);
            let new_dedup_id = new_attributes.get(ROJO_DEDUP_KEY);
            if let (Some(old_dedup_id), Some(new_dedup_id)) = (old_dedup_id, new_dedup_id) {
                if old_dedup_id != new_dedup_id {
                    diff_score += 10;
                } else {
                    same_score += 10;
                }
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

        // let mut children_by_name: HashMap<&str, i32> = HashMap::new();
        // for child_ref in old_instance.children() {
        //     let child = self.old_tree.get_by_ref(*child_ref).unwrap();
        //     diff_score += 1;
        //     *children_by_name.entry(&child.name).or_default() += 1;
        // }
        // for child_ref in new_instance.children() {
        //     let child = self.new_tree.get_by_ref(*child_ref).unwrap();
        //     diff_score += 1;
        //     children_by_name.entry(&child.name).and_modify(|v| {
        //         if *v > 0 {
        //             same_score += 1;
        //             diff_score -= 2; // subtract diff added by both loops
        //             *v -= 1;
        //         }
        //     });
        // }

        // let mut children_props: BTreeMap<u64, i32> = BTreeMap::new();
        // for child_ref in old_instance.children() {
        //     let child = self.old_tree.get_by_ref(*child_ref).unwrap();

        //     for (property_name, value) in child.properties.iter() {
        //         let mut hasher = DefaultHasher::new();
        //         child.name.hash(&mut hasher);
        //         property_name.hash(&mut hasher);
        //         value.hash(&mut hasher);
        //         let hash = hasher.finish();

        //         diff_score += 1;
        //         *children_props.entry(hash).or_default() += 1;
        //     }
        // }
        // for child_ref in new_instance.children() {
        //     let child = self.new_tree.get_by_ref(*child_ref).unwrap();

        //     for (property_name, value) in child.properties.iter() {
        //         let mut hasher = DefaultHasher::new();
        //         child.name.hash(&mut hasher);
        //         property_name.hash(&mut hasher);
        //         value.hash(&mut hasher);
        //         let hash = hasher.finish();

        //         diff_score += 1;
        //         children_props.entry(hash).and_modify(|v| {
        //             if *v > 0 {
        //                 same_score += 1;
        //                 diff_score -= 2; // subtract diff added by both loops
        //                 *v -= 1;
        //             }
        //         });
        //     }
        // }

        (diff_score, same_score)
    }
}

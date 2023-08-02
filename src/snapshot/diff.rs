use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Display,
    ops::AddAssign,
};

use rbx_dom_weak::{
    types::{Ref, Variant},
    Instance, WeakDom,
};

use super::{default_filters_diff, PropertiesFiltered, PropertyFilter, WeakDomExtra};

pub fn diff_properties<'a>(
    old_instance: &'a Instance,
    new_instance: &'a Instance,
    filters: &'a BTreeMap<String, PropertyFilter>,
) -> Box<dyn Iterator<Item = String> + 'a> {
    Box::new(
        old_instance
            .properties_filtered(filters, true)
            .chain(new_instance.properties_filtered(filters, true))
            .filter_map(|(key, _)| {
                let new_prop = new_instance.properties.get(key);
                let old_prop = old_instance.properties.get(key);

                if old_prop == new_prop {
                    None
                } else {
                    Some(key.to_owned())
                }
            }),
    )
}

pub fn are_properties_different(
    old_instance: &Instance,
    new_instance: &Instance,
    filters: &BTreeMap<String, PropertyFilter>,
) -> bool {
    diff_properties(old_instance, new_instance, filters).any(|_| true)
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
    pub property_refs: HashSet<Ref>,
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
            diff: &self,
            old_tree: &old_tree,
            new_tree: &new_tree,
        }
    }

    pub fn has_changed_descendants(&self, old_ref: Ref) -> bool {
        self.changed_children.contains_key(&old_ref)
    }

    pub fn has_changed_properties(&self, old_ref: Ref) -> bool {
        self.changed.contains_key(&old_ref)
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
        self.property_refs.contains(&referent)
    }

    pub fn get_children(
        &self,
        old_tree: &WeakDom,
        new_tree: &WeakDom,
        old_ref: Ref,
    ) -> Option<(HashSet<Ref>, HashSet<Ref>, HashSet<Ref>, HashSet<Ref>)> {
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

    fn prune_matching_ref_properties(&mut self, old_tree: &WeakDom, new_tree: &WeakDom) -> () {
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
                            if self.new_to_old[new_ref] == *old_ref {
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
                self.unmark_ancestors(old_tree, old_ref)
            }
        }
    }

    fn move_ref(&mut self, new_dom_ref: Ref, replacement: Ref) -> () {
        if let Some(old_ref) = self.new_to_old.remove(&new_dom_ref) {
            self.new_to_old.insert(replacement, old_ref);

            if self.changed.remove(&old_ref).is_some() {
                self.changed.insert(old_ref, replacement);
            } else if self.unchanged.remove(&old_ref).is_some() {
                self.unchanged.insert(old_ref, replacement);
            }
        }

        if self.property_refs.remove(&new_dom_ref) {
            self.property_refs.insert(replacement);
        }
    }

    fn deduplicate_refs(&mut self, old_tree: &WeakDom, new_tree: &mut WeakDom) -> () {
        let mut ref_map = BTreeMap::new();
        {
            // Fix duplicate refs
            for referent in new_tree.descendants().collect::<Vec<_>>() {
                if old_tree.get_by_ref(referent).is_some()
                    && self.get_matching_old_ref(referent) != Some(referent)
                {
                    let replacement = loop {
                        let replacement = Ref::new();
                        if old_tree.get_by_ref(replacement).is_none() {
                            break replacement;
                        }
                    };
                    new_tree.swap_ref(referent, replacement);
                    self.move_ref(referent, replacement);
                    ref_map.insert(referent, replacement);
                }
            }
        }
        {
            // Fix properties
            for referent in new_tree.descendants().collect::<Vec<_>>() {
                for (k, v) in new_tree
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

    fn find_ref_properties(&mut self, old_tree: &WeakDom, new_tree: &WeakDom) -> () {
        for referent in old_tree.descendants() {
            for (k, v) in old_tree.get_by_ref(referent).unwrap().properties.iter() {
                if let Variant::Ref(prop_ref) = v {
                    self.property_refs.insert(*prop_ref);
                }
            }
        }

        for referent in new_tree.descendants() {
            for (k, v) in new_tree.get_by_ref(referent).unwrap().properties.iter() {
                if let Variant::Ref(prop_ref) = v {
                    self.property_refs.insert(*prop_ref);
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
                    let (diff_score, same_score) =
                        similarity_score(old_tree, old_child, new_tree, new_child, filters);

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

    fn mark_ancestors(&mut self, old_tree: &WeakDom, ancestor_ref: Ref) {
        // log::trace!("marking ancestors for {}", ancestor_ref);
        let mut ancestor_ref = ancestor_ref;
        loop {
            match old_tree.get_by_ref(ancestor_ref) {
                Some(old_inst) => {
                    if self.changed_children.contains_key(&ancestor_ref) {
                        self.changed_children
                            .entry(ancestor_ref)
                            .and_modify(|v| *v += 1);
                        break;
                    }

                    self.changed_children.insert(ancestor_ref, 1);
                    ancestor_ref = old_inst.parent();
                }
                None => break,
            }
        }
    }

    fn unmark_ancestors(&mut self, old_tree: &WeakDom, ancestor_ref: Ref) {
        // log::trace!("unmarking ancestors for {}", ancestor_ref);
        let mut ancestor_ref = ancestor_ref;
        loop {
            match old_tree.get_by_ref(ancestor_ref) {
                Some(old_inst) => {
                    if !self.changed_children.contains_key(&ancestor_ref) {
                        break;
                    }

                    let changed_children = self.changed_children[&ancestor_ref] - 1;
                    if changed_children != 0 {
                        self.changed_children.insert(ancestor_ref, changed_children);
                        break;
                    }

                    self.changed_children.remove(&ancestor_ref);
                    ancestor_ref = old_inst.parent();
                }
                None => break,
            }
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

        while !process.is_empty() {
            let (old_ref, new_ref) = process.pop().unwrap();
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

pub fn similarity_score(
    old_tree: &WeakDom,
    old_instance: &Instance,
    new_tree: &WeakDom,
    new_instance: &Instance,
    filters: &BTreeMap<String, PropertyFilter>,
) -> (i32, i32) {
    let mut diff_score = 0;
    let mut same_score = 0;

    let old_properties = old_instance.properties_filtered_map(filters, true);

    let new_properties = new_instance.properties_filtered_map(filters, true);

    for (k, v) in old_properties.iter() {
        if new_properties.contains_key(k) && new_properties[k] == *v {
            same_score += 1;
        } else {
            diff_score += 1;
        }
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
        let child = old_tree.get_by_ref(*child_ref).unwrap();
        diff_score += 1;
        children_by_name
            .entry(&child.name)
            .or_default()
            .add_assign(1);
    }
    for child_ref in new_instance.children() {
        let child = new_tree.get_by_ref(*child_ref).unwrap();
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

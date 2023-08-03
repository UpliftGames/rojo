use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::OnceLock,
};

use rbx_dom_weak::{
    types::{Attributes, BinaryString, Ref, SharedString, Tags, Variant},
    Instance, WeakDom,
};
use serde::{Deserialize, Serialize};

use super::InstanceWithMeta;

pub fn default_filters_diff() -> &'static BTreeMap<String, PropertyFilter> {
    static VALUE: OnceLock<BTreeMap<String, PropertyFilter>> = OnceLock::new();

    VALUE.get_or_init(|| {
        BTreeMap::from_iter(
            [
                ("SourceAssetId", PropertyFilter::Ignore),
                ("UniqueId", PropertyFilter::Ignore),
                ("ScriptGuid", PropertyFilter::Ignore),
                ("HistoryId", PropertyFilter::Ignore),
                (
                    "PrimaryPart",
                    PropertyFilter::IgnoreWhenEq(vec![Variant::Ref(Ref::none())]),
                ),
                (
                    "Tags",
                    PropertyFilter::IgnoreWhenEq(vec![Variant::Tags(Tags::default())]),
                ),
                (
                    "Attributes",
                    PropertyFilter::IgnoreWhenEq(vec![Variant::Attributes(Attributes::new())]),
                ),
                (
                    "MaterialVariantSerialized",
                    PropertyFilter::IgnoreWhenEq(vec![
                        Variant::SharedString(SharedString::new(vec![])),
                        Variant::BinaryString(BinaryString::new()),
                    ]),
                ),
                (
                    "ModelMeshData",
                    PropertyFilter::IgnoreWhenEq(vec![
                        Variant::SharedString(SharedString::new(vec![])),
                        Variant::BinaryString(BinaryString::new()),
                    ]),
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v)),
        )
    })
}

pub fn default_filters_save() -> &'static BTreeMap<String, PropertyFilter> {
    static VALUE: OnceLock<BTreeMap<String, PropertyFilter>> = OnceLock::new();

    VALUE.get_or_init(|| {
        BTreeMap::from_iter(
            [
                ("SourceAssetId", PropertyFilter::Ignore),
                ("UniqueId", PropertyFilter::Ignore),
                ("ScriptGuid", PropertyFilter::Ignore),
                ("HistoryId", PropertyFilter::Ignore),
                (
                    "PrimaryPart",
                    PropertyFilter::IgnoreWhenEq(vec![Variant::Ref(Ref::none())]),
                ),
                (
                    "Tags",
                    PropertyFilter::IgnoreWhenEq(vec![Variant::Tags(Tags::default())]),
                ),
                (
                    "Attributes",
                    PropertyFilter::IgnoreWhenEq(vec![Variant::Attributes(Attributes::new())]),
                ),
                (
                    "MaterialVariantSerialized",
                    PropertyFilter::IgnoreWhenEq(vec![
                        Variant::SharedString(SharedString::new(vec![])),
                        Variant::BinaryString(BinaryString::new()),
                    ]),
                ),
                (
                    "ModelMeshData",
                    PropertyFilter::IgnoreWhenEq(vec![
                        Variant::SharedString(SharedString::new(vec![])),
                        Variant::BinaryString(BinaryString::new()),
                    ]),
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v)),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropertyFilter {
    Ignore,
    IgnoreWhenEq(Vec<Variant>),
}

pub trait PropertyFilterTrait {
    fn should_ignore(&self, value: &Variant) -> bool;
}

impl PropertyFilterTrait for PropertyFilter {
    fn should_ignore(&self, value: &Variant) -> bool {
        match self {
            PropertyFilter::Ignore => true,
            PropertyFilter::IgnoreWhenEq(values) => values.iter().any(|v| v == value),
        }
    }
}

impl PropertyFilterTrait for Option<&PropertyFilter> {
    fn should_ignore(&self, value: &Variant) -> bool {
        match self {
            Some(filter) => filter.should_ignore(value),
            None => false,
        }
    }
}

pub fn filter<'a>(
    class: &'a str,
    filters: &'a BTreeMap<String, PropertyFilter>,
    filter_defaults: bool,
) -> Box<dyn FnMut(&(&str, &Variant)) -> bool + 'a> {
    Box::new(move |(k, v)| {
        if filter_defaults {
            let default = rbx_reflection_database::get()
                .classes
                .get(class)
                .map(|class_def| class_def.default_properties.get(*k))
                .flatten();
            if default == Some(v) {
                return false;
            }
        }
        if let Some(filter) = filters.get(*k) {
            match filter {
                PropertyFilter::Ignore => return false,
                PropertyFilter::IgnoreWhenEq(values) => {
                    for filter_value in values {
                        if v == &filter_value {
                            return false;
                        }
                    }
                }
            }
        }

        true
    })
}

pub trait PropertiesFiltered {
    fn properties_iter(&self) -> Box<dyn Iterator<Item = (&str, &Variant)> + '_>;
    fn class_inner(&self) -> &str;

    fn properties_filtered<'a>(
        &'a self,
        filters: &'a BTreeMap<String, PropertyFilter>,
        filter_defaults: bool,
    ) -> Box<dyn Iterator<Item = (&str, &Variant)> + 'a> {
        Box::new(self.properties_iter().filter(filter(
            self.class_inner(),
            filters,
            filter_defaults,
        )))
    }

    fn properties_filtered_map<'a>(
        &'a self,
        filters: &'a BTreeMap<String, PropertyFilter>,
        filter_defaults: bool,
    ) -> BTreeMap<&str, &Variant> {
        self.properties_filtered(filters, filter_defaults).collect()
    }
}

impl PropertiesFiltered for Instance {
    fn class_inner(&self) -> &str {
        self.class.as_str()
    }
    fn properties_iter(&self) -> Box<dyn Iterator<Item = (&str, &Variant)> + '_> {
        Box::new(self.properties.iter().map(|(k, v)| (k.as_str(), v)))
    }
}

impl PropertiesFiltered for InstanceWithMeta<'_> {
    fn class_inner(&self) -> &str {
        self.class_name()
    }
    fn properties_iter(&self) -> Box<dyn Iterator<Item = (&str, &Variant)> + '_> {
        Box::new(self.properties().iter().map(|(k, v)| (k.as_str(), v)))
    }
}

pub trait ToVariantBinaryString {
    fn to_variant_binary_string(&self) -> Option<Variant>;
}

impl ToVariantBinaryString for Variant {
    fn to_variant_binary_string(&self) -> Option<Variant> {
        match self {
            Variant::BinaryString(s) => Some(Variant::BinaryString(s.clone())),
            Variant::SharedString(s) => Some(Variant::BinaryString(BinaryString::from(s.data()))),
            Variant::String(s) => Some(Variant::BinaryString(BinaryString::from(s.as_ref()))),
            _ => None,
        }
    }
}

pub trait ToVariantUnicodeString {
    fn to_variant_unicode_string(&self) -> Option<Variant>;
}

impl ToVariantUnicodeString for Variant {
    fn to_variant_unicode_string(&self) -> Option<Variant> {
        match self {
            Variant::BinaryString(s) => core::str::from_utf8(s.as_ref())
                .map_or(None, |s| Some(Variant::String(s.to_owned()))),
            Variant::SharedString(s) => {
                core::str::from_utf8(s.data()).map_or(None, |s| Some(Variant::String(s.to_owned())))
            }
            Variant::String(s) => Some(Variant::String(s.to_owned())),
            _ => None,
        }
    }
}

pub trait InstanceExtra {
    fn get_attributes(&mut self) -> &Attributes;
    fn get_attributes_mut(&mut self) -> &mut Attributes;
}

impl InstanceExtra for Instance {
    fn get_attributes(&mut self) -> &Attributes {
        let attributes = self
            .properties
            .entry("Attributes".to_string())
            .and_modify(|attributes| {
                match attributes {
                    Variant::Attributes(_) => (),
                    _ => *attributes = Variant::Attributes(Attributes::new()),
                };
            })
            .or_insert(Variant::Attributes(Attributes::new()));

        match self.properties.get("Attributes").unwrap() {
            Variant::Attributes(attributes) => attributes,
            _ => unreachable!(),
        }
    }
    fn get_attributes_mut(&mut self) -> &mut Attributes {
        let attributes = self
            .properties
            .entry("Attributes".to_string())
            .and_modify(|attributes| {
                match attributes {
                    Variant::Attributes(_) => (),
                    _ => *attributes = Variant::Attributes(Attributes::new()),
                };
            })
            .or_insert(Variant::Attributes(Attributes::new()));

        match attributes {
            Variant::Attributes(attributes) => attributes,
            _ => unreachable!(),
        }
    }
}

pub trait WeakDomExtra {
    fn descendants(&self) -> Box<dyn Iterator<Item = Ref> + '_>;
    fn descendants_of(&self, ancestor: Ref) -> Box<dyn Iterator<Item = Ref> + '_>;

    fn deduplicate_refs(&mut self, other: &Self) -> BTreeMap<Ref, Ref>;

    fn mark_external_refs(&mut self, ancestor: Ref, global_prop_refs: &HashSet<Ref>) -> ();
    fn apply_marked_external_refs(&mut self, ancestor: Ref) -> ();
}

impl WeakDomExtra for WeakDom {
    fn descendants(&self) -> Box<dyn Iterator<Item = Ref> + '_> {
        self.descendants_of(self.root_ref())
    }
    fn descendants_of(&self, ancestor: Ref) -> Box<dyn Iterator<Item = Ref> + '_> {
        let mut processing = vec![ancestor];

        Box::new(std::iter::from_fn(move || loop {
            let referent = processing.pop()?;
            let instance = self.get_by_ref(referent);
            let instance = match instance {
                Some(instance) => instance,
                None => continue,
            };
            processing.extend(instance.children());
            return Some(referent);
        }))
    }

    fn deduplicate_refs(&mut self, other: &Self) -> BTreeMap<Ref, Ref> {
        let mut ref_map = BTreeMap::new();
        {
            // Fix duplicate refs
            for referent in self.descendants().collect::<Vec<_>>() {
                if other.get_by_ref(referent).is_some() {
                    let new_ref = loop {
                        let new_ref = Ref::new();
                        if other.get_by_ref(new_ref).is_none() {
                            break new_ref;
                        }
                    };
                    self.swap_ref(referent, new_ref);
                    ref_map.insert(referent, new_ref);
                }
            }
        }
        {
            // Fix properties
            for referent in self.descendants().collect::<Vec<_>>() {
                for (_k, v) in self.get_by_ref_mut(referent).unwrap().properties.iter_mut() {
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

    fn mark_external_refs(&mut self, ancestor: Ref, global_prop_refs: &HashSet<Ref>) -> () {
        let refs: BTreeSet<Ref> = self.descendants_of(ancestor).collect();
        let mut my_prop_refs: BTreeSet<Ref> = BTreeSet::new();
        for referent in refs.iter() {
            let instance = self.get_by_ref_mut(*referent).unwrap();
            let mut external_refs = BTreeMap::new();
            for (k, v) in instance.properties.iter_mut() {
                if let Variant::Ref(prop_ref) = v {
                    if prop_ref.is_some() && !refs.contains(prop_ref) {
                        external_refs.insert(k.clone(), *prop_ref);
                        log::trace!(
                            "Found external ref prop {} under {}: {}",
                            k,
                            instance.name,
                            *prop_ref
                        );
                    } else {
                        my_prop_refs.insert(*prop_ref);
                    }
                }
            }
            if !external_refs.is_empty() {
                let attributes = instance.get_attributes_mut();
                attributes.insert(
                    "RojoExternalRefProps".to_string(),
                    Variant::String(serde_json::to_string(&external_refs).unwrap()),
                );
            }
        }

        for referent in refs.iter() {
            if global_prop_refs.contains(referent) && !my_prop_refs.contains(referent) {
                log::trace!(
                    "{} is used externally as a property, storing {} ref",
                    self.get_by_ref(*referent).unwrap().name,
                    referent
                );
                let attributes = self.get_by_ref_mut(*referent).unwrap().get_attributes_mut();
                attributes.insert(
                    "RojoExternalRef".to_string(),
                    Variant::String(serde_json::to_string(&referent).unwrap()),
                );
            }
        }
    }

    fn apply_marked_external_refs(&mut self, ancestor: Ref) -> () {
        let refs: BTreeSet<Ref> = self.descendants_of(ancestor).collect();
        for referent in refs.iter() {
            let name;
            let (external_ref, external_ref_props) = {
                let instance = self.get_by_ref_mut(*referent).unwrap();
                name = instance.name.clone();
                let attributes = instance.get_attributes_mut();
                (
                    attributes
                        .remove("RojoExternalRef")
                        .map(|v| v.to_variant_binary_string())
                        .flatten(),
                    attributes
                        .remove("RojoExternalRefProps")
                        .map(|v| v.to_variant_binary_string())
                        .flatten(),
                )
            };

            if let Some(Variant::BinaryString(external_ref_props)) = external_ref_props {
                let instance = self.get_by_ref_mut(*referent).unwrap();
                let external_ref_props =
                    serde_json::from_slice::<BTreeMap<String, Ref>>(external_ref_props.as_ref())
                        .unwrap();
                for (k, v) in external_ref_props {
                    log::trace!("Applying external ref prop {} to {}: {}", k, &name, v);
                    instance.properties.insert(k, v.into());
                }
            }
            if let Some(Variant::BinaryString(external_ref)) = external_ref {
                let external_ref = serde_json::from_slice::<Ref>(external_ref.as_ref()).unwrap();
                log::trace!("Applying external ref to {}: {}", &name, external_ref);
                self.swap_ref(*referent, external_ref);
            }
        }
    }
}

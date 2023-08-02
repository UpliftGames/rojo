use std::{collections::BTreeMap, sync::OnceLock};

use rbx_dom_weak::{
    types::{BinaryString, Ref, Tags, Variant},
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
                    "Tags",
                    PropertyFilter::IgnoreWhenEq(vec![Variant::Tags(Tags::default())]),
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
                    "Tags",
                    PropertyFilter::IgnoreWhenEq(vec![Variant::Tags(Tags::default())]),
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
        Box::new(self.properties_iter().filter_map(move |(k, v)| {
            if filter_defaults {
                let default = rbx_reflection_database::get()
                    .classes
                    .get(self.class_inner())
                    .map(|class_def| class_def.default_properties.get(k))
                    .flatten();
                if default == Some(v) {
                    return None;
                }
            }
            if let Some(filter) = filters.get(k) {
                match filter {
                    PropertyFilter::Ignore => return None,
                    PropertyFilter::IgnoreWhenEq(values) => {
                        for filter_value in values {
                            if v == filter_value {
                                return None;
                            }
                        }
                    }
                }
            }

            Some((k, v))
        }))
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

pub trait WeakDomExtra {
    fn descendants(&self) -> Box<dyn Iterator<Item = Ref> + '_>;
    fn deduplicate_refs(&mut self, other: &Self) -> BTreeMap<Ref, Ref>;
}

impl WeakDomExtra for WeakDom {
    fn descendants(&self) -> Box<dyn Iterator<Item = Ref> + '_> {
        let mut processing = vec![self.root_ref()];

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
                for (k, v) in self.get_by_ref_mut(referent).unwrap().properties.iter_mut() {
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
}

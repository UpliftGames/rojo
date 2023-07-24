use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use rbx_dom_weak::{
    types::{BinaryString, Tags, Variant},
    Instance,
};

use super::InstanceWithMeta;

pub fn ignore_keys_cmp() -> &'static HashSet<&'static str> {
    static VALUE: OnceLock<HashSet<&'static str>> = OnceLock::new();

    VALUE.get_or_init(|| HashSet::from(["SourceAssetId", "UniqueId", "ScriptGuid", "HistoryId"]))
}

pub fn ignore_pairs_cmp() -> &'static HashMap<&'static str, Variant> {
    static VALUE: OnceLock<HashMap<&'static str, Variant>> = OnceLock::new();

    VALUE.get_or_init(|| HashMap::from([("Tags", Variant::Tags(Tags::default()))]))
}
pub fn ignore_keys_save() -> &'static HashSet<&'static str> {
    static VALUE: OnceLock<HashSet<&'static str>> = OnceLock::new();

    VALUE.get_or_init(|| HashSet::from(["SourceAssetId", "ScriptGuid", "HistoryId"]))
}

pub fn ignore_pairs_save() -> &'static HashMap<&'static str, Variant> {
    static VALUE: OnceLock<HashMap<&'static str, Variant>> = OnceLock::new();

    VALUE.get_or_init(|| HashMap::from([("Tags", Variant::Tags(Tags::default()))]))
}

pub trait PropertiesFiltered {
    fn properties_iter(&self) -> Box<dyn Iterator<Item = (&str, &Variant)> + '_>;
    fn class_inner(&self) -> &str;

    fn properties_filtered_cmp(&self) -> Box<dyn Iterator<Item = (&str, &Variant)> + '_> {
        Box::new(self.properties_iter().filter_map(|(k, v)| {
            let default = rbx_reflection_database::get()
                .classes
                .get(self.class_inner())
                .map(|class_def| class_def.default_properties.get(k))
                .flatten();
            if default == Some(v) {
                None
            } else if ignore_keys_cmp().contains(k) {
                None
            } else if ignore_pairs_cmp().get(k) == Some(v) {
                None
            } else {
                Some((k, v))
            }
        }))
    }
    fn properties_filtered_cmp_map(&self) -> HashMap<&str, &Variant> {
        self.properties_filtered_cmp().collect()
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

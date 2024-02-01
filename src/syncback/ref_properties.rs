//! Implements iterating through an entire WeakDom and linking all Ref
//! properties using attributes.

use std::collections::VecDeque;

use rbx_dom_weak::{
    types::{Attributes, BinaryString, Variant},
    Instance, WeakDom,
};

use crate::{multimap::MultiMap, REF_ID_ATTRIBUTE_NAME, REF_POINTER_ATTRIBUTE_PREFIX};

/// Iterates through a WeakDom and links referent properties using attributes.
pub fn link_referents(dom: &mut WeakDom) -> anyhow::Result<()> {
    let mut links = MultiMap::new();
    let mut queue = VecDeque::new();

    // Note that this is back-in, front-out. This is important because
    // VecDeque::extend is the equivalent to using push_back.
    queue.push_back(dom.root_ref());
    while let Some(referent) = queue.pop_front() {
        {
            let attributes = get_or_insert_attributes(dom.get_by_ref_mut(referent).unwrap())?;
            if attributes.get(REF_ID_ATTRIBUTE_NAME).is_none() {
                let bstring = BinaryString::from(referent.to_string().into_bytes());
                attributes.insert(REF_ID_ATTRIBUTE_NAME.into(), bstring.into());
            }
        }
        let instance = dom.get_by_ref(referent).unwrap();

        queue.extend(instance.children().iter().copied());
        for (name, value) in &instance.properties {
            if let Variant::Ref(prop_value) = value {
                if dom.get_by_ref(*prop_value).is_some() {
                    links.insert(referent, (name.clone(), *prop_value))
                }
            }
        }
    }
    let mut rewrites = Vec::new();

    for (referent, ref_properties) in links {
        for (prop_name, target_ref) in ref_properties {
            let target_id = get_attributes(dom.get_by_ref(target_ref).unwrap())
                .unwrap()
                .get(REF_ID_ATTRIBUTE_NAME)
                .expect("every Instance to have an ID");
            if let Variant::BinaryString(target_id) = target_id {
                rewrites.push((prop_name, target_id.clone()));
            }
        }
        let inst = dom.get_by_ref_mut(referent).unwrap();
        let attrs = get_or_insert_attributes(inst)?;
        for (name, id) in rewrites.drain(..) {
            attrs.insert(
                format!("{REF_POINTER_ATTRIBUTE_PREFIX}{name}"),
                BinaryString::from(id.into_vec()).into(),
            );
        }
    }

    Ok(())
}

fn get_attributes(inst: &Instance) -> Option<&Attributes> {
    match inst.properties.get("Attributes") {
        Some(Variant::Attributes(attrs)) => Some(attrs),
        _ => None,
    }
}

fn get_or_insert_attributes(inst: &mut Instance) -> anyhow::Result<&mut Attributes> {
    if !inst.properties.contains_key("Attributes") {
        inst.properties
            .insert("Attributes".into(), Attributes::new().into());
    }
    match inst.properties.get_mut("Attributes") {
        Some(Variant::Attributes(attrs)) => Ok(attrs),
        Some(ty) => Err(anyhow::format_err!(
            "expected property Attributes to be an Attributes but it was {:?}",
            ty.ty()
        )),
        None => unreachable!(),
    }
}

//! Property paths: naming one field deep inside a reflected value, reading
//! it and writing it. Prefab overrides are stored this way.
//!
//! A path is the field names joined by dots, as in `health.max`. List items
//! are numbered (`slots.1`), a list's length is `slots.#len`, and the value
//! inside an `Option` is `attack.?` (the option itself, at `attack`, reads
//! as [`Value::None`] or [`Value::Some`]).

use reign_core::{Fx, FxVec2};
use serde::{Deserialize, Serialize};

use crate::ObjectId;
use crate::reflect::{Mut, Reflect, Rgba8};

/// One leaf of a reflected value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Fx(Fx),
    Vec2(FxVec2),
    Color(Rgba8),
    Str(String),
    Object(ObjectId),
    /// An asset handle's GUID, as 32 hex digits ("" for none).
    Asset(String),
    /// A fieldless enum's variant, by name.
    Enum(String),
    /// An `Option` that is `None`.
    None,
    /// An `Option` that is `Some` (its value has paths of its own).
    Some,
    /// A `Vec`'s length.
    Len(u32),
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_owned() } else { format!("{prefix}.{name}") }
}

/// Every leaf under `value`, in a fixed order: fields in declaration
/// order, an option's state before its inside, a list's length before its
/// items. Applying them in this order rebuilds the value.
pub fn leaves(value: &mut dyn Reflect, prefix: &str, out: &mut Vec<(String, Value)>) {
    let leaf = |v: Value| (prefix.to_owned(), v);
    match value.reflect_mut() {
        Mut::Bool(b) => out.push(leaf(Value::Bool(*b))),
        Mut::Int(i) => out.push(leaf(Value::Int(i.get() as i64))),
        Mut::Fx(f) => out.push(leaf(Value::Fx(*f))),
        Mut::FxVec2(v) => out.push(leaf(Value::Vec2(*v))),
        Mut::Color(c) => out.push(leaf(Value::Color(*c))),
        Mut::String(s) => out.push(leaf(Value::Str(s.clone()))),
        Mut::Object(id) => out.push(leaf(Value::Object(*id))),
        Mut::Asset(a) => {
            let guid = a.guid();
            let text = if guid == 0 { String::new() } else { format!("{guid:032x}") };
            out.push(leaf(Value::Asset(text)));
        }
        Mut::Enum(e) => {
            let name = e.variants().get(e.variant_index()).copied().unwrap_or("");
            out.push(leaf(Value::Enum(name.to_owned())));
        }
        Mut::Option(o) => match o.get_mut() {
            None => out.push(leaf(Value::None)),
            Some(inner) => {
                out.push(leaf(Value::Some));
                leaves(inner, &join(prefix, "?"), out);
            }
        },
        Mut::List(l) => {
            if l.resizable() {
                out.push((join(prefix, "#len"), Value::Len(l.len() as u32)));
            }
            for i in 0..l.len() {
                if let Some(item) = l.get_mut(i) {
                    leaves(item, &join(prefix, &i.to_string()), out);
                }
            }
        }
        Mut::Struct(s) => {
            for i in 0..s.fields().len() {
                let name = s.fields()[i].name;
                if let Some(field) = s.field_mut(i) {
                    leaves(field, &join(prefix, name), out);
                }
            }
        }
    }
}

/// The value at `path`, if there is one.
pub fn get(value: &mut dyn Reflect, path: &str) -> Option<Value> {
    let mut all = Vec::new();
    leaves(value, "", &mut all);
    all.into_iter().find(|(p, _)| p == path).map(|(_, v)| v)
}

/// Sets the leaf at `path`. Options and lists along the way are created
/// as needed. False if the path doesn't fit the value's shape.
pub fn set(value: &mut dyn Reflect, path: &str, to: &Value) -> bool {
    let segments: Vec<&str> = if path.is_empty() { Vec::new() } else { path.split('.').collect() };
    set_at(value, &segments, to)
}

fn set_at(value: &mut dyn Reflect, path: &[&str], to: &Value) -> bool {
    match (value.reflect_mut(), path) {
        (Mut::Option(o), []) => match to {
            Value::None => {
                o.set_none();
                true
            }
            Value::Some => {
                o.set_some_default();
                true
            }
            _ => false,
        },
        (Mut::Option(o), ["?", rest @ ..]) => {
            o.set_some_default();
            o.get_mut().is_some_and(|inner| set_at(inner, rest, to))
        }
        (Mut::List(l), ["#len"]) => {
            let Value::Len(n) = to else { return false };
            if !l.resizable() {
                return l.len() == *n as usize;
            }
            while l.len() > *n as usize {
                l.remove(l.len() - 1);
            }
            while l.len() < *n as usize {
                l.push_default();
            }
            true
        }
        (Mut::List(l), [index, rest @ ..]) => {
            let Ok(i) = index.parse::<usize>() else { return false };
            if l.resizable() {
                while l.len() <= i {
                    l.push_default();
                }
            }
            l.get_mut(i).is_some_and(|item| set_at(item, rest, to))
        }
        (Mut::Struct(s), [name, rest @ ..]) => {
            s.field_by_name(name).is_some_and(|(_, field)| set_at(field, rest, to))
        }
        (leaf, []) => set_leaf(leaf, to),
        _ => false,
    }
}

fn set_leaf(leaf: Mut<'_>, to: &Value) -> bool {
    match (leaf, to) {
        (Mut::Bool(b), Value::Bool(v)) => *b = *v,
        (Mut::Int(mut i), Value::Int(v)) => i.set(*v as i128),
        (Mut::Fx(f), Value::Fx(v)) => *f = *v,
        (Mut::FxVec2(f), Value::Vec2(v)) => *f = *v,
        (Mut::Color(c), Value::Color(v)) => *c = *v,
        (Mut::String(s), Value::Str(v)) => s.clone_from(v),
        (Mut::Object(id), Value::Object(v)) => *id = *v,
        (Mut::Asset(a), Value::Asset(v)) => {
            let guid = if v.is_empty() { 0 } else { u128::from_str_radix(v, 16).unwrap_or(0) };
            a.set_guid(guid);
        }
        (Mut::Enum(e), Value::Enum(v)) => {
            let Some(i) = e.variants().iter().position(|n| n == v) else { return false };
            e.set_variant_index(i);
        }
        _ => return false,
    }
    true
}

/// Rewrites every `ObjectId` inside `value` through `map`.
pub fn remap_objects(value: &mut dyn Reflect, map: &mut dyn FnMut(ObjectId) -> ObjectId) {
    match value.reflect_mut() {
        Mut::Object(id) => *id = map(*id),
        Mut::Option(o) => {
            if let Some(inner) = o.get_mut() {
                remap_objects(inner, map);
            }
        }
        Mut::List(l) => {
            for i in 0..l.len() {
                if let Some(item) = l.get_mut(i) {
                    remap_objects(item, map);
                }
            }
        }
        Mut::Struct(s) => {
            for i in 0..s.fields().len() {
                if let Some(field) = s.field_mut(i) {
                    remap_objects(field, map);
                }
            }
        }
        _ => {}
    }
}

/// The paths where `a` and `b` differ, with `b`'s values, in an order that
/// rebuilds `b` from `a` when applied with [`set`].
pub fn diff(a: &mut dyn Reflect, b: &mut dyn Reflect) -> Vec<(String, Value)> {
    let (mut la, mut lb) = (Vec::new(), Vec::new());
    leaves(a, "", &mut la);
    leaves(b, "", &mut lb);
    lb.into_iter().filter(|(path, v)| !la.iter().any(|(p, w)| p == path && w == v)).collect()
}

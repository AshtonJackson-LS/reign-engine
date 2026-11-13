//! Reflection through the derives: every supported field kind can be read
//! and written without knowing the type.

mod common;

use common::{Everything, Inner, Mood};
use reign_core::{Fx, FxVec2};
use reign_scene::reflect::{ReflectEnum, ReflectStruct};
use reign_scene::{Bounds, Component, Mut, ObjectId, Reflect, Rgba8, Transform};

fn struct_of(value: &mut dyn Reflect) -> &mut dyn ReflectStruct {
    let name = value.type_name();
    match value.reflect_mut() {
        Mut::Struct(s) => s,
        _ => panic!("{name} is not a struct"),
    }
}

fn field<'a>(s: &'a mut dyn ReflectStruct, name: &str) -> &'a mut dyn Reflect {
    s.field_by_name(name).unwrap_or_else(|| panic!("no field {name}")).1
}

#[test]
fn derive_lists_fields_with_their_attributes() {
    let mut e = Everything::default();
    let s = struct_of(&mut e);
    let names: Vec<_> = s.fields().iter().map(|f| f.name).collect();
    assert_eq!(
        names,
        [
            "small", "health", "big", "lit", "speed", "offset", "tint", "name", "mood", "target",
            "cooldown", "path", "slots", "inner", "ticks"
        ],
        "skipped fields are left out"
    );
    let health = s.fields()[1];
    assert_eq!(health.range, Some((0.0, 100.0)));
    assert_eq!(health.tooltip, Some("Hit points."));
    assert_eq!(health.type_name, "u16");
    assert_eq!(s.fields()[3].tooltip, Some("Whether it's lit."), "doc comments become tooltips");
    assert!(s.fields()[14].read_only);
    assert_eq!(Everything::NAME, "Everything");
    assert_eq!(e.bounds(), Some(Bounds::new(-8, -16, 16, 16)));
}

#[test]
fn every_field_kind_can_be_set_through_reflection() {
    let mut e = Everything::default();
    let s = struct_of(&mut e);
    for (name, value) in [("small", -200), ("health", 42), ("big", -1 << 40)] {
        let Mut::Int(mut int) = field(s, name).reflect_mut() else { panic!("{name}") };
        int.set(value);
    }
    let Mut::Bool(lit) = field(s, "lit").reflect_mut() else { panic!() };
    *lit = true;
    let Mut::Fx(speed) = field(s, "speed").reflect_mut() else { panic!() };
    *speed = Fx::from_raw(-3);
    let Mut::FxVec2(offset) = field(s, "offset").reflect_mut() else { panic!() };
    *offset = FxVec2::from_px(1, -2);
    let Mut::Color(tint) = field(s, "tint").reflect_mut() else { panic!() };
    *tint = Rgba8::rgb(1, 2, 3);
    let Mut::String(name) = field(s, "name").reflect_mut() else { panic!() };
    name.push_str("lantern");
    let Mut::Enum(mood) = field(s, "mood").reflect_mut() else { panic!() };
    assert_eq!(mood.variants(), ["Calm", "Angry", "Sleepy"]);
    mood.set_variant_index(2);
    let Mut::Option(target) = field(s, "target").reflect_mut() else { panic!() };
    target.set_some_default();
    let Mut::Object(id) = target.get_mut().unwrap().reflect_mut() else { panic!() };
    *id = ObjectId::new(4, 1);
    let Mut::Option(cooldown) = field(s, "cooldown").reflect_mut() else { panic!() };
    cooldown.set_some_default();
    cooldown.set_none();
    let Mut::List(path) = field(s, "path").reflect_mut() else { panic!() };
    assert!(path.resizable());
    path.push_default();
    path.push_default();
    let Mut::FxVec2(p) = path.get_mut(1).unwrap().reflect_mut() else { panic!() };
    p.x = Fx::ONE;
    path.remove(0);
    let Mut::List(slots) = field(s, "slots").reflect_mut() else { panic!() };
    assert!(!slots.resizable());
    slots.push_default();
    assert_eq!(slots.len(), 3, "arrays can't grow");
    let Mut::Int(mut slot) = slots.get_mut(2).unwrap().reflect_mut() else { panic!() };
    slot.set(300);
    let inner = struct_of(field(s, "inner"));
    let Mut::Int(mut depth) = field(inner, "depth").reflect_mut() else { panic!() };
    depth.set(-5);
    let Mut::String(label) = field(inner, "label").reflect_mut() else { panic!() };
    *label = "deep".into();

    assert_eq!(
        e,
        Everything {
            small: i8::MIN,
            health: 42,
            big: -1 << 40,
            lit: true,
            speed: Fx::from_raw(-3),
            offset: FxVec2::from_px(1, -2),
            tint: Rgba8::rgb(1, 2, 3),
            name: "lantern".into(),
            mood: Mood::Sleepy,
            target: Some(ObjectId::new(4, 1)),
            cooldown: None,
            path: vec![FxVec2 { x: Fx::ONE, y: Fx::ZERO }],
            slots: [0, 0, 255],
            inner: Inner { depth: 0, label: "deep".into() },
            ticks: 0,
            cache: 0,
        },
        "integers clamp to their type"
    );
}

#[test]
fn reading_through_reflection() {
    let mut e = Everything { health: 9, mood: Mood::Angry, ..Everything::default() };
    let s = struct_of(&mut e);
    let Mut::Int(health) = field(s, "health").reflect_mut() else { panic!() };
    assert_eq!(health.get(), 9);
    assert_eq!(health.bounds(), (0, u16::MAX as i128));
    let Mut::Enum(mood) = field(s, "mood").reflect_mut() else { panic!() };
    assert_eq!(mood.variant_index(), 1);
    assert_eq!(field(s, "inner").type_name(), "Inner");
    let mut transform = Transform::default();
    let t = struct_of(&mut transform);
    assert_eq!(t.fields().len(), 4);
    assert!(t.fields()[1].tooltip.is_some());
}

#[test]
fn enums_reflect_their_variants() {
    let mut mood = Mood::Calm;
    mood.set_variant_index(1);
    assert_eq!(mood, Mood::Angry);
    mood.set_variant_index(99);
    assert_eq!(mood, Mood::Angry, "out of range does nothing");
}

#[test]
fn every_field_kind_serializes_and_reads_back() {
    let sample = Everything::sample();
    let text = ron::to_string(&sample).unwrap();
    assert!(text.contains("speed:\"-0.01171875\""), "{text}");
    let back: Everything = ron::from_str(&text).unwrap();
    // The skipped-by-reflection field still round-trips through serde.
    assert_eq!(back, sample);
}

//! Reflection: what a component's fields are, so the editor can show and
//! edit them without knowing the type.
//!
//! Every reflected value says what kind of thing it is through
//! [`Reflect::reflect_mut`], which hands back a typed mutable view
//! ([`Mut`]). Structs list their fields with [`FieldInfo`] (name, type,
//! attributes) and give each one out as another `&mut dyn Reflect`, so
//! nested data is walked the same way all the way down.
//!
//! `#[derive(Reflect)]` and `#[derive(Component)]` write these impls; the
//! ones here cover the leaf types: integers, `bool`, [`Fx`], [`FxVec2`],
//! [`Rgba8`], `String`, [`ObjectId`], `Option<T>`, `Vec<T>` and arrays.

use std::any::Any;

use reign_core::{Fx, FxVec2};
use serde::{Deserialize, Serialize};

use crate::ObjectId;

/// What the derive knows about a field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldInfo {
    pub name: &'static str,
    /// The field's type as written in the source.
    pub type_name: &'static str,
    /// `#[reflect(read_only)]`: shown, never edited.
    pub read_only: bool,
    /// `#[reflect(range(min, max))]`: editors clamp to this. Editor-side
    /// metadata only; the simulation never sees the floats.
    pub range: Option<(f64, f64)>,
    /// `#[reflect(tooltip = "...")]`, or the field's doc comment.
    pub tooltip: Option<&'static str>,
}

impl FieldInfo {
    pub const fn new(name: &'static str, type_name: &'static str) -> Self {
        FieldInfo { name, type_name, read_only: false, range: None, tooltip: None }
    }
}

/// A value the editor can inspect and change.
pub trait Reflect: Any {
    /// The type's short name, for labels.
    fn type_name(&self) -> &'static str;

    /// A typed mutable view of the value.
    fn reflect_mut(&mut self) -> Mut<'_>;

    fn as_any(&self) -> &dyn Any;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// What kind of value a [`Reflect`] is, with mutable access to it.
pub enum Mut<'a> {
    Bool(&'a mut bool),
    Int(IntMut<'a>),
    Fx(&'a mut Fx),
    FxVec2(&'a mut FxVec2),
    Color(&'a mut Rgba8),
    String(&'a mut String),
    Object(&'a mut ObjectId),
    /// A handle to an asset, by GUID (see `reign_assets::Handle`).
    Asset(&'a mut dyn ReflectAsset),
    Enum(&'a mut dyn ReflectEnum),
    Option(&'a mut dyn ReflectOption),
    List(&'a mut dyn ReflectList),
    Struct(&'a mut dyn ReflectStruct),
}

/// A struct's fields, in declaration order.
pub trait ReflectStruct {
    fn fields(&self) -> &'static [FieldInfo];

    /// The field at `index` in [`ReflectStruct::fields`].
    fn field_mut(&mut self, index: usize) -> Option<&mut dyn Reflect>;
}

impl dyn ReflectStruct + '_ {
    /// The field called `name`, with its info.
    pub fn field_by_name(&mut self, name: &str) -> Option<(FieldInfo, &mut dyn Reflect)> {
        let index = self.fields().iter().position(|f| f.name == name)?;
        let info = self.fields()[index];
        self.field_mut(index).map(|f| (info, f))
    }
}

/// A typed asset handle: a GUID and the kind of asset it must point at.
/// The scene crate doesn't know asset types; `reign_assets` implements this
/// for its handles.
pub trait ReflectAsset {
    /// The GUID as a number (0 for none).
    fn guid(&self) -> u128;
    fn set_guid(&mut self, guid: u128);
    /// The asset kind's name, such as `"Texture"`.
    fn asset_kind(&self) -> &'static str;
}

/// A fieldless enum: one of a fixed list of variants.
pub trait ReflectEnum {
    fn variants(&self) -> &'static [&'static str];
    fn variant_index(&self) -> usize;
    /// Switches to the variant at `index`; out of range does nothing.
    fn set_variant_index(&mut self, index: usize);
}

/// An `Option<T>`.
pub trait ReflectOption {
    fn get_mut(&mut self) -> Option<&mut dyn Reflect>;
    fn set_none(&mut self);
    /// Makes it `Some(T::default())` if it was `None`.
    fn set_some_default(&mut self);
}

/// A `Vec<T>` or `[T; N]`.
pub trait ReflectList {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn get_mut(&mut self, index: usize) -> Option<&mut dyn Reflect>;
    /// Whether items can be added and removed (`Vec`, not arrays).
    fn resizable(&self) -> bool;
    fn push_default(&mut self);
    fn remove(&mut self, index: usize);
}

/// A colour as 8-bit sRGB channels. Scene state stays free of floats, so
/// colours are stored like this and converted for drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    pub const WHITE: Rgba8 = Rgba8::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba8 { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba8 { r, g, b, a }
    }
}

macro_rules! ints {
    ($($variant:ident $ty:ty),* $(,)?) => {
        /// A mutable integer of any width, read and written as `i128`.
        pub enum IntMut<'a> {
            $($variant(&'a mut $ty),)*
        }

        impl IntMut<'_> {
            pub fn get(&self) -> i128 {
                match self {
                    $(IntMut::$variant(v) => **v as i128,)*
                }
            }

            /// Sets the value, clamped to what the type can hold.
            pub fn set(&mut self, value: i128) {
                match self {
                    $(IntMut::$variant(v) => {
                        **v = value.clamp(<$ty>::MIN as i128, <$ty>::MAX as i128) as $ty
                    })*
                }
            }

            /// The smallest and largest values the type holds.
            pub fn bounds(&self) -> (i128, i128) {
                match self {
                    $(IntMut::$variant(_) => (<$ty>::MIN as i128, <$ty>::MAX as i128),)*
                }
            }
        }

        $(impl Reflect for $ty {
            fn type_name(&self) -> &'static str {
                stringify!($ty)
            }
            fn reflect_mut(&mut self) -> Mut<'_> {
                Mut::Int(IntMut::$variant(self))
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }
        })*
    };
}

ints!(I8 i8, I16 i16, I32 i32, I64 i64, U8 u8, U16 u16, U32 u32, U64 u64, Usize usize, Isize isize);

macro_rules! leaf {
    ($ty:ty, $variant:ident, $name:literal) => {
        impl Reflect for $ty {
            fn type_name(&self) -> &'static str {
                $name
            }
            fn reflect_mut(&mut self) -> Mut<'_> {
                Mut::$variant(self)
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }
        }
    };
}

leaf!(bool, Bool, "bool");
leaf!(Fx, Fx, "Fx");
leaf!(FxVec2, FxVec2, "FxVec2");
leaf!(Rgba8, Color, "Rgba8");
leaf!(String, String, "String");
leaf!(ObjectId, Object, "ObjectId");

impl<T: Reflect + Default> Reflect for Option<T> {
    fn type_name(&self) -> &'static str {
        "Option"
    }
    fn reflect_mut(&mut self) -> Mut<'_> {
        Mut::Option(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl<T: Reflect + Default> ReflectOption for Option<T> {
    fn get_mut(&mut self) -> Option<&mut dyn Reflect> {
        self.as_mut().map(|v| v as &mut dyn Reflect)
    }
    fn set_none(&mut self) {
        *self = None;
    }
    fn set_some_default(&mut self) {
        if self.is_none() {
            *self = Some(T::default());
        }
    }
}

impl<T: Reflect + Default> Reflect for Vec<T> {
    fn type_name(&self) -> &'static str {
        "Vec"
    }
    fn reflect_mut(&mut self) -> Mut<'_> {
        Mut::List(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl<T: Reflect + Default> ReflectList for Vec<T> {
    fn len(&self) -> usize {
        Vec::len(self)
    }
    fn get_mut(&mut self, index: usize) -> Option<&mut dyn Reflect> {
        <[T]>::get_mut(self, index).map(|v| v as &mut dyn Reflect)
    }
    fn resizable(&self) -> bool {
        true
    }
    fn push_default(&mut self) {
        self.push(T::default());
    }
    fn remove(&mut self, index: usize) {
        if index < Vec::len(self) {
            Vec::remove(self, index);
        }
    }
}

impl<T: Reflect, const N: usize> Reflect for [T; N] {
    fn type_name(&self) -> &'static str {
        "array"
    }
    fn reflect_mut(&mut self) -> Mut<'_> {
        Mut::List(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl<T: Reflect, const N: usize> ReflectList for [T; N] {
    fn len(&self) -> usize {
        N
    }
    fn get_mut(&mut self, index: usize) -> Option<&mut dyn Reflect> {
        <[T]>::get_mut(self, index).map(|v| v as &mut dyn Reflect)
    }
    fn resizable(&self) -> bool {
        false
    }
    fn push_default(&mut self) {}
    fn remove(&mut self, _index: usize) {}
}

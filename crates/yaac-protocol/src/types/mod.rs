//! Every wire type of the YAAC protocol (host ⇄ `yaacd`).
//!
//! Field names are camelCase on the wire. Unknown fields are ignored on decode. Every enum is
//! *open*: values this build does not know are preserved (see [`crate::macros`]), never an
//! error. These types are the version-free model the rest of the program uses; the
//! [`crate::dialect`] layer translates them to and from the wire at the negotiated version.

mod capability;
mod event;
mod forge;
mod fs;
mod ids;
mod input;
mod item;
mod journal;
mod limits;
mod mesh;
mod project;
mod provider;
mod request;
mod session;
mod thread;
mod upload;

pub use capability::*;
pub use event::*;
pub use forge::*;
pub use fs::*;
pub use ids::*;
pub use input::*;
pub use item::*;
pub use journal::*;
pub use limits::*;
pub use mesh::*;
pub use project::*;
pub use provider::*;
pub use request::*;
pub use session::*;
pub use thread::*;
pub use upload::*;

use serde_json::{Map, Value};

/// A tagged-union variant this build does not know, kept exactly as received (tag included).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UnknownVariant {
    /// Every field of the object, including the tag.
    pub fields: Map<String, Value>,
}

impl UnknownVariant {
    /// The value of the tag field, or `""` when it is missing (it never is after decoding).
    pub fn tag(&self, tag_field: &str) -> &str {
        self.fields.get(tag_field).and_then(Value::as_str).unwrap_or("")
    }
}

/// An empty JSON object, for methods that take or return nothing (`{}` on the wire).
///
/// Kept as an object rather than `null` so fields can be added later without a version bump.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct Empty {}

/// Serde helper for patch fields: absent = `None`, `null` = `Some(None)`, value = `Some(Some)`.
/// Use with `#[serde(default, skip_serializing_if = "Option::is_none", with = "…")]`.
pub(crate) mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<T: Serialize, S: Serializer>(
        value: &Option<Option<T>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

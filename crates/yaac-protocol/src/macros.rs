//! Macros that give every wire type the same forward-compatibility behaviour.
//!
//! * [`id_newtype!`] — an opaque string newtype (ids, tokens, names).
//! * [`open_enum!`] — a string enum whose unknown values are preserved in a fallback variant
//!   instead of failing to decode.
//! * [`open_union!`] — an internally tagged union (`{"type": "text", …}`) whose unknown tags are
//!   preserved as the raw JSON object, so they round-trip byte-for-byte in meaning.
//!
//! The JSON Schema these macros emit marks the fallback with the `x-yaac-unknown` keyword. The
//! TypeScript generator leaves such members out of the generated unions (they cannot be typed),
//! and documents that a newer peer may send them.

/// An opaque string newtype, transparent on the wire.
macro_rules! id_newtype {
    ($(#[doc = $doc:literal])* $name:ident) => {
        $(#[doc = $doc])*
        #[derive(
            Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default,
            ::serde::Serialize, ::serde::Deserialize, ::schemars::JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Wraps a string.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            /// The wire string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
            /// Unwraps into the wire string.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl ::std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

/// A string enum that keeps values it does not know.
///
/// ```ignore
/// open_enum! {
///     /// Doc.
///     pub enum ThreadState fallback Unknown {
///         /// Doc.
///         Idle = "idle",
///     }
/// }
/// ```
///
/// The fallback variant holds the unknown wire string. `from_wire` always prefers a known
/// variant, so decoding never produces the fallback for a known string.
macro_rules! open_enum {
    (
        $(#[doc = $doc:literal])*
        pub enum $name:ident fallback $fallback:ident {
            $(
                $(#[doc = $vdoc:literal])*
                $variant:ident = $wire:literal
            ),+ $(,)?
        }
    ) => {
        $(#[doc = $doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $(
                $(#[doc = $vdoc])*
                $variant,
            )+
            /// A value this build does not know. Kept as sent, so it round-trips.
            $fallback(String),
        }

        impl $name {
            /// Every value this build knows, in declaration order.
            pub const KNOWN: &'static [$name] = &[$($name::$variant),+];

            /// The wire string.
            pub fn as_str(&self) -> &str {
                match self {
                    $($name::$variant => $wire,)+
                    $name::$fallback(other) => other.as_str(),
                }
            }

            /// Decodes a wire string. Never fails: unknown strings become the fallback variant.
            pub fn from_wire(value: &str) -> Self {
                match value {
                    $($wire => $name::$variant,)+
                    other => $name::$fallback(other.to_owned()),
                }
            }

            /// Whether this build knows the value.
            pub fn is_known(&self) -> bool {
                !matches!(self, $name::$fallback(_))
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ::std::convert::Infallible;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self::from_wire(s))
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::from_wire(value)
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct V;
                impl ::serde::de::Visitor<'_> for V {
                    type Value = $name;
                    fn expecting(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                        f.write_str("a string")
                    }
                    fn visit_str<E: ::serde::de::Error>(self, v: &str) -> Result<$name, E> {
                        Ok($name::from_wire(v))
                    }
                }
                deserializer.deserialize_str(V)
            }
        }

        impl ::schemars::JsonSchema for $name {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }
            fn json_schema(_: &mut ::schemars::SchemaGenerator) -> ::schemars::Schema {
                let description = $crate::macros::doc_text(concat!($($doc, "\n"),*));
                let mut schema = ::schemars::json_schema!({
                    "anyOf": [
                        { "type": "string", "enum": [$($wire),+] },
                        { "type": "string", "x-yaac-unknown": true }
                    ]
                });
                if !description.is_empty() {
                    schema.insert("description".into(), description.into());
                }
                schema
            }
        }
    };
}

/// An internally tagged union that keeps variants it does not know.
///
/// ```ignore
/// open_union! {
///     /// Doc.
///     pub enum InputPart tag "type" {
///         /// Doc.
///         Text(TextPart) = "text",
///     }
/// }
/// ```
///
/// Each known variant wraps a struct that serialises as a JSON object; the tag is added on
/// encode and ignored by the struct on decode. An unknown tag decodes into
/// `Unknown(UnknownVariant)`, which re-encodes to exactly the object received.
macro_rules! open_union {
    (
        $(#[doc = $doc:literal])*
        pub enum $name:ident tag $tag:literal {
            $(
                $(#[doc = $vdoc:literal])*
                $variant:ident($body:ty) = $wire:literal
            ),+ $(,)?
        }
    ) => {
        $(#[doc = $doc])*
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum $name {
            $(
                $(#[doc = $vdoc])*
                $variant($body),
            )+
            /// A variant this build does not know. Kept as received, so it round-trips.
            Unknown($crate::types::UnknownVariant),
        }

        impl $name {
            /// The name of the tag field on the wire.
            pub const TAG: &'static str = $tag;
            /// Every tag value this build knows.
            pub const KNOWN_TAGS: &'static [&'static str] = &[$($wire),+];

            /// The tag value of this variant.
            pub fn tag(&self) -> &str {
                match self {
                    $($name::$variant(_) => $wire,)+
                    $name::Unknown(unknown) => unknown.tag($tag),
                }
            }

            /// Whether this build knows the variant.
            pub fn is_known(&self) -> bool {
                !matches!(self, $name::Unknown(_))
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use ::serde::ser::{Error as _, SerializeMap as _};
                let (tag, fields) = match self {
                    $(
                        $name::$variant(body) => {
                            match ::serde_json::to_value(body).map_err(S::Error::custom)? {
                                ::serde_json::Value::Object(map) => ($wire, map),
                                _ => return Err(S::Error::custom(concat!(
                                    stringify!($body), " must serialise as a JSON object"
                                ))),
                            }
                        }
                    )+
                    $name::Unknown(unknown) => {
                        let mut map = serializer.serialize_map(Some(unknown.fields.len()))?;
                        for (key, value) in &unknown.fields {
                            map.serialize_entry(key, value)?;
                        }
                        return map.end();
                    }
                };
                let mut map = serializer.serialize_map(Some(fields.len() + 1))?;
                map.serialize_entry($tag, tag)?;
                for (key, value) in &fields {
                    if key != $tag {
                        map.serialize_entry(key, value)?;
                    }
                }
                map.end()
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                use ::serde::de::Error as _;
                let fields = ::serde_json::Map::<String, ::serde_json::Value>::deserialize(deserializer)?;
                let tag = match fields.get($tag) {
                    Some(::serde_json::Value::String(tag)) => tag.clone(),
                    Some(_) => return Err(D::Error::custom(concat!(
                        "the `", $tag, "` field of ", stringify!($name), " must be a string"
                    ))),
                    None => return Err(D::Error::missing_field($tag)),
                };
                match tag.as_str() {
                    $(
                        $wire => ::serde_json::from_value(::serde_json::Value::Object(fields))
                            .map($name::$variant)
                            .map_err(D::Error::custom),
                    )+
                    _ => Ok($name::Unknown($crate::types::UnknownVariant { fields })),
                }
            }
        }

        impl ::schemars::JsonSchema for $name {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }
            fn json_schema(generator: &mut ::schemars::SchemaGenerator) -> ::schemars::Schema {
                let description = $crate::macros::doc_text(concat!($($doc, "\n"),*));
                let mut members: Vec<::serde_json::Value> = Vec::new();
                $(
                    let body = generator.subschema_for::<$body>();
                    members.push(::serde_json::json!({
                        "allOf": [
                            {
                                "type": "object",
                                "properties": { $tag: { "type": "string", "const": $wire } },
                                "required": [$tag]
                            },
                            body
                        ]
                    }));
                )+
                members.push(::serde_json::json!({
                    "type": "object",
                    "properties": { $tag: { "type": "string" } },
                    "required": [$tag],
                    "x-yaac-unknown": true
                }));
                let mut schema = ::schemars::json_schema!({ "anyOf": members });
                if !description.is_empty() {
                    schema.insert("description".into(), description.into());
                }
                schema
            }
        }
    };
}

/// Turns concatenated `#[doc]` lines into description text, the way rustdoc would.
pub(crate) fn doc_text(raw: &str) -> String {
    raw.lines()
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

pub(crate) use {id_newtype, open_enum, open_union};

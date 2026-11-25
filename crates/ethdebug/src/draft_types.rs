//! Type objects of the draft type schema, ethdebug/format pull request #424.
//!
//! The released 0.1.0-draft.3 type schemas describe a `uint` with a bit width
//! and no way to say that a type is a variant or that a mapping has a key. The
//! pull request replaces them. The documents written here keep the stamp of the
//! released version, and the type objects follow revision [`REVISION`] of the
//! pull request. This module is the only place that spells a kind or a field
//! name of those type objects; if the draft changes, only this file does.
//!
//! A type is either written in place ([`TypeRef::Inline`]) or named by an id
//! that the resources document defines ([`TypeRef::Id`], key `t<N>` of
//! `types`). A type that mentions itself needs the second form.

use serde_json::{Map, Value, json};

/// The commit of the pull request head that the type objects follow.
pub const REVISION: &str = "e326f17117fbde383d33293d717236a65c394e97";

/// What `compilation.settings.ethdebugTypes` says, so that a reader of the
/// documents alone can tell which draft the type objects follow.
pub const TYPES_DRAFT: &str = "ethdebug/format#424 at e326f17117fbde383d33293d717236a65c394e97";

/// The `settings` of the compilation object.
pub(crate) fn compilation_settings() -> Value {
    json!({ "ethdebugTypes": TYPES_DRAFT })
}

/// A type, in place or by id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Inline(Box<TypeDesc>),
    /// The entry `t<N>` of the resources document's `types`.
    Id(u32),
}

/// The name and, when known, the source range of a type's definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    /// Source id, offset and length.
    pub location: Option<(u32, u32, u32)>,
}

/// One member of a struct. A struct without named members is a tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: Option<String>,
    pub ty: TypeRef,
}

/// One case of a variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub name: String,
    pub discriminant: u64,
    pub payload: Option<TypeRef>,
}

/// A type object. `label` is the type as the source language spells it, for
/// display only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDesc {
    /// An integer of unstated width, read from the bytes of its location.
    Number {
        label: String,
    },
    Bool {
        label: String,
    },
    /// Bytes, in order, of unstated length.
    Bytes {
        label: String,
    },
    String {
        label: String,
    },
    /// A plain address: no `definition` and no statement about payability.
    Address {
        label: String,
    },
    Struct {
        label: Option<String>,
        definition: Option<Definition>,
        members: Vec<Member>,
    },
    /// A value that is exactly one of its cases. No value is valid for a
    /// variant without cases.
    Variant {
        label: Option<String>,
        definition: Option<Definition>,
        cases: Vec<Case>,
    },
    Mapping {
        label: String,
        key: TypeRef,
        value: TypeRef,
    },
    Array {
        label: String,
        element: TypeRef,
    },
    /// A function called within the language, with unnamed parameters.
    Function {
        label: String,
        parameters: Vec<TypeRef>,
        returns: Vec<TypeRef>,
    },
    Alias {
        label: Option<String>,
        definition: Option<Definition>,
        target: TypeRef,
    },
}

/// The key of type `n` in the resources document's `types`.
pub fn type_key(n: u32) -> String {
    format!("t{n}")
}

impl TypeRef {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Inline(desc) => desc.to_json(),
            Self::Id(n) => json!({ "id": type_key(*n) }),
        }
    }

    fn wrapper(&self) -> Value {
        json!({ "type": self.to_json() })
    }
}

impl Definition {
    fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("name".to_owned(), json!(self.name));
        if let Some((source, offset, length)) = self.location {
            out.insert(
                "location".to_owned(),
                json!({ "source": { "id": source }, "range": { "offset": offset, "length": length } }),
            );
        }
        Value::Object(out)
    }
}

impl TypeDesc {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Number { label } => elementary("number", label),
            Self::Bool { label } => elementary("bool", label),
            Self::Bytes { label } => elementary("bytes", label),
            Self::String { label } => elementary("string", label),
            Self::Address { label } => elementary("address", label),
            Self::Struct {
                label,
                definition,
                members,
            } => {
                let members: Vec<Value> = members
                    .iter()
                    .map(|member| {
                        let mut out = Map::new();
                        if let Some(name) = &member.name {
                            out.insert("name".to_owned(), json!(name));
                        }
                        out.insert("type".to_owned(), member.ty.to_json());
                        Value::Object(out)
                    })
                    .collect();
                let mut out = complex("struct", false, label.as_deref(), definition.as_ref());
                out.insert("contains".to_owned(), Value::Array(members));
                Value::Object(out)
            }
            Self::Variant {
                label,
                definition,
                cases,
            } => {
                let cases: Vec<Value> = cases
                    .iter()
                    .map(|case| {
                        let mut out = Map::new();
                        out.insert("name".to_owned(), json!(case.name));
                        out.insert("discriminant".to_owned(), json!(case.discriminant));
                        if let Some(payload) = &case.payload {
                            out.insert("payload".to_owned(), payload.wrapper());
                        }
                        Value::Object(out)
                    })
                    .collect();
                // A variant states its parts in `cases`, not in `contains`, so
                // `class` says that it is complex.
                let mut out = complex("variant", true, label.as_deref(), definition.as_ref());
                out.insert("cases".to_owned(), Value::Array(cases));
                Value::Object(out)
            }
            Self::Mapping { label, key, value } => {
                let mut out = complex("mapping", false, Some(label), None);
                out.insert(
                    "contains".to_owned(),
                    json!({ "key": key.wrapper(), "value": value.wrapper() }),
                );
                Value::Object(out)
            }
            Self::Array { label, element } => {
                let mut out = complex("array", false, Some(label), None);
                out.insert("contains".to_owned(), element.wrapper());
                Value::Object(out)
            }
            Self::Function {
                label,
                parameters,
                returns,
            } => {
                let mut out = complex("function", true, Some(label), None);
                out.insert("internal".to_owned(), json!(true));
                out.insert(
                    "parameters".to_owned(),
                    Value::Array(parameters.iter().map(TypeRef::wrapper).collect()),
                );
                out.insert(
                    "returns".to_owned(),
                    Value::Array(returns.iter().map(TypeRef::wrapper).collect()),
                );
                Value::Object(out)
            }
            Self::Alias {
                label,
                definition,
                target,
            } => {
                let mut out = complex("alias", false, label.as_deref(), definition.as_ref());
                out.insert("contains".to_owned(), target.wrapper());
                Value::Object(out)
            }
        }
    }
}

fn elementary(kind: &str, label: &str) -> Value {
    json!({ "kind": kind, "label": label })
}

fn complex(
    kind: &str,
    with_class: bool,
    label: Option<&str>,
    definition: Option<&Definition>,
) -> Map<String, Value> {
    let mut out = Map::new();
    if with_class {
        out.insert("class".to_owned(), json!("complex"));
    }
    out.insert("kind".to_owned(), json!(kind));
    if let Some(label) = label {
        out.insert("label".to_owned(), json!(label));
    }
    if let Some(definition) = definition {
        out.insert("definition".to_owned(), definition.to_json());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(label: &str) -> TypeRef {
        TypeRef::Inline(Box::new(TypeDesc::Number {
            label: label.to_owned(),
        }))
    }

    #[test]
    fn variant_names_its_class_and_wraps_payloads_and_references() {
        let shape = TypeDesc::Variant {
            label: Some("Shape".to_owned()),
            definition: Some(Definition {
                name: "Shape".to_owned(),
                location: Some((0, 64, 71)),
            }),
            cases: vec![
                Case {
                    name: "Dot".to_owned(),
                    discriminant: 0,
                    payload: Some(number("uint256")),
                },
                Case {
                    name: "Seg".to_owned(),
                    discriminant: 1,
                    payload: Some(TypeRef::Id(2)),
                },
                Case {
                    name: "Nothing".to_owned(),
                    discriminant: 2,
                    payload: None,
                },
            ],
        };
        assert_eq!(
            shape.to_json(),
            json!({
                "class": "complex",
                "kind": "variant",
                "label": "Shape",
                "definition": {
                    "name": "Shape",
                    "location": { "source": { "id": 0 }, "range": { "offset": 64, "length": 71 } },
                },
                "cases": [
                    { "name": "Dot", "discriminant": 0,
                      "payload": { "type": { "kind": "number", "label": "uint256" } } },
                    { "name": "Seg", "discriminant": 1, "payload": { "type": { "id": "t2" } } },
                    { "name": "Nothing", "discriminant": 2 },
                ],
            })
        );
        let empty = TypeDesc::Variant {
            label: None,
            definition: None,
            cases: Vec::new(),
        };
        assert_eq!(
            empty.to_json(),
            json!({ "class": "complex", "kind": "variant", "cases": [] })
        );
    }

    #[test]
    fn struct_mapping_array_function_and_alias_give_their_parts_as_wrappers() {
        let tuple = TypeDesc::Struct {
            label: None,
            definition: None,
            members: vec![
                Member {
                    name: None,
                    ty: number("uint256"),
                },
                Member {
                    name: Some("flag".to_owned()),
                    ty: TypeRef::Inline(Box::new(TypeDesc::Bool {
                        label: "bool".to_owned(),
                    })),
                },
            ],
        };
        assert_eq!(
            tuple.to_json(),
            json!({
                "kind": "struct",
                "contains": [
                    { "type": { "kind": "number", "label": "uint256" } },
                    { "name": "flag", "type": { "kind": "bool", "label": "bool" } },
                ],
            })
        );
        let mapping = TypeDesc::Mapping {
            label: "mapping(address => uint256)".to_owned(),
            key: TypeRef::Inline(Box::new(TypeDesc::Address {
                label: "address".to_owned(),
            })),
            value: number("uint256"),
        };
        assert_eq!(
            mapping.to_json(),
            json!({
                "kind": "mapping",
                "label": "mapping(address => uint256)",
                "contains": {
                    "key": { "type": { "kind": "address", "label": "address" } },
                    "value": { "type": { "kind": "number", "label": "uint256" } },
                },
            })
        );
        let array = TypeDesc::Array {
            label: "array<word>".to_owned(),
            element: TypeRef::Inline(Box::new(TypeDesc::Bytes {
                label: "word".to_owned(),
            })),
        };
        assert_eq!(
            array.to_json(),
            json!({
                "kind": "array",
                "label": "array<word>",
                "contains": { "type": { "kind": "bytes", "label": "word" } },
            })
        );
        let function = TypeDesc::Function {
            label: "function(uint256) returns (())".to_owned(),
            parameters: vec![number("uint256")],
            returns: Vec::new(),
        };
        assert_eq!(
            function.to_json(),
            json!({
                "class": "complex",
                "kind": "function",
                "label": "function(uint256) returns (())",
                "internal": true,
                "parameters": [{ "type": { "kind": "number", "label": "uint256" } }],
                "returns": [],
            })
        );
        let alias = TypeDesc::Alias {
            label: Some("Amount".to_owned()),
            definition: Some(Definition {
                name: "Amount".to_owned(),
                location: None,
            }),
            target: TypeRef::Id(0),
        };
        assert_eq!(
            alias.to_json(),
            json!({
                "kind": "alias",
                "label": "Amount",
                "definition": { "name": "Amount" },
                "contains": { "type": { "id": "t0" } },
            })
        );
    }

    #[test]
    fn the_revision_is_part_of_the_note() {
        assert!(TYPES_DRAFT.ends_with(REVISION));
        assert_eq!(
            compilation_settings(),
            json!({ "ethdebugTypes": TYPES_DRAFT })
        );
    }
}

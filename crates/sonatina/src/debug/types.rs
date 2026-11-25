//! The description of an instantiated source type in the draft type format.
//!
//! One structural rule covers user data types: one constructor makes a struct,
//! two make a variant, none makes a variant without cases, and three or more
//! have no description (see `build_user_adt`). A short, closed list of std
//! types has a fixed meaning instead, because std defines them as one word and
//! the source language gives that word a meaning (a number, an address). A
//! type that the rules cannot describe has no description, and so has every
//! type that contains one: a wrong description is worse than none.
//!
//! This module decides which type is which and never spells JSON; the draft's
//! kinds and fields live in `ethdebug::draft_types`.

use std::collections::{HashMap, HashSet};

use ethdebug::draft_types::{Case, Definition, Member};
use ethdebug::{TypeDesc, TypeRef};
use hir::{
    anchor::DefId,
    ast::item::{AdtDef, ContractItem, Item, Module},
    nameres::ident_text,
    span::Span,
};
use hir_ty::{
    BuiltinTyCtor, Ty, TyCtor, TyKind, UserTyCtorKind, display_ty, is_canonical_std_def_named,
    is_canonical_std_module_def_named,
};
use hull::origin::SourceTable;
use parser::parse_file_to_hir;

/// What describing a type needs to know besides the type.
#[derive(Clone, Copy)]
pub struct Env<'a, 'db> {
    pub db: &'db dyn hir_ty::Db,
    /// Maps the file of a definition to the id its source has in the debug files.
    pub sources: &'a SourceTable,
}

/// The std unsigned integer types besides `uint256`, which live in `std/unsigned.sol`.
const UNSIGNED: [&str; 5] = ["uint8", "uint16", "uint32", "uint64", "uint128"];

/// What a table entry stands for. A constructor payload with several fields has
/// no type of its own in the language, so it is keyed by its constructor.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key<'db> {
    Ty(Ty<'db>),
    Payload(Ty<'db>, u32),
}

/// The complex type descriptions of one compilation, by deterministic id.
///
/// Elementary descriptions are written in place and never enter the table. A
/// complex description is stored once and referred to by id, which also lets a
/// type that contains itself be described without unbounded expansion. Ids
/// follow the order in which types are first met, members depth first.
#[derive(Default)]
pub struct TypeTable<'db> {
    descs: Vec<TypeDesc>,
    ids: HashMap<Key<'db>, u32>,
    failed: HashSet<Key<'db>>,
}

enum Shape {
    /// Described in place.
    Elementary(TypeDesc),
    /// Described by a table entry.
    Complex,
    Undescribable,
}

impl<'db> TypeTable<'db> {
    /// The complex descriptions; entry `n` has the id `n`.
    pub fn descriptions(&self) -> &[TypeDesc] {
        &self.descs
    }

    /// How `ty` is written where it is used, or `None` when it has no description.
    pub fn describe(&mut self, env: &Env<'_, 'db>, ty: Ty<'db>) -> Option<TypeRef> {
        match shape(env, ty) {
            Shape::Elementary(desc) => Some(TypeRef::Inline(Box::new(desc))),
            Shape::Complex => self.entry(env, Key::Ty(ty)),
            Shape::Undescribable => None,
        }
    }

    /// The id of the entry for `key`, adding it first if needed. A type under
    /// construction already has its id, so a member that names it needs no
    /// second description. When a description fails, everything added since it
    /// was started goes away, and the failure is remembered: it does not depend
    /// on where the type was met.
    fn entry(&mut self, env: &Env<'_, 'db>, key: Key<'db>) -> Option<TypeRef> {
        if let Some(id) = self.ids.get(&key) {
            return Some(TypeRef::Id(*id));
        }
        if self.failed.contains(&key) {
            return None;
        }
        let id = self.descs.len() as u32;
        self.descs.push(TypeDesc::Struct {
            label: None,
            definition: None,
            members: Vec::new(),
        });
        self.ids.insert(key, id);
        match self.build(env, key) {
            Some(desc) => {
                self.descs[id as usize] = desc;
                Some(TypeRef::Id(id))
            }
            None => {
                self.descs.truncate(id as usize);
                self.ids.retain(|_, kept| *kept < id);
                self.failed.insert(key);
                None
            }
        }
    }

    fn build(&mut self, env: &Env<'_, 'db>, key: Key<'db>) -> Option<TypeDesc> {
        match key {
            Key::Ty(ty) => self.build_type(env, ty),
            Key::Payload(ty, ctor) => {
                let (def, args) = user_adt(env, ty)?;
                let view = adt_view(env.db, def, args)?;
                let members = self.members(env, &view.ctors.get(ctor as usize)?.fields, None)?;
                Some(TypeDesc::Struct {
                    label: None,
                    definition: None,
                    members,
                })
            }
        }
    }

    fn members(
        &mut self,
        env: &Env<'_, 'db>,
        fields: &[Ty<'db>],
        names: Option<&[String]>,
    ) -> Option<Vec<Member>> {
        fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                Some(Member {
                    name: names.map(|names| names[index].clone()),
                    ty: self.describe(env, *field)?,
                })
            })
            .collect()
    }

    fn build_type(&mut self, env: &Env<'_, 'db>, ty: Ty<'db>) -> Option<TypeDesc> {
        let db = env.db;
        let label = display_ty(db, ty);
        match ty.kind(db) {
            TyKind::Tuple(elems) => Some(TypeDesc::Struct {
                label: Some(label),
                definition: None,
                members: self.members(env, elems, None)?,
            }),
            TyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
                ..
            } => Some(TypeDesc::Struct {
                label: Some(label),
                definition: None,
                members: Vec::new(),
            }),
            TyKind::Named {
                ctor: TyCtor::Builtin(BuiltinTyCtor::Sum),
                args,
            } => {
                let [left, right] = args.as_slice() else {
                    return None;
                };
                let case = |name: &str, discriminant, payload| Case {
                    name: name.to_owned(),
                    discriminant,
                    payload: Some(payload),
                };
                let inl = case("inl", 0, self.describe(env, *left)?);
                let inr = case("inr", 1, self.describe(env, *right)?);
                Some(TypeDesc::Variant {
                    label: Some(label),
                    definition: None,
                    cases: vec![inl, inr],
                })
            }
            TyKind::Function { params, ret } => {
                let parameters = params
                    .iter()
                    .map(|param| self.describe(env, *param))
                    .collect::<Option<Vec<_>>>()?;
                let returns = if is_unit(db, *ret) {
                    Vec::new()
                } else {
                    vec![self.describe(env, *ret)?]
                };
                Some(TypeDesc::Function {
                    label,
                    parameters,
                    returns,
                })
            }
            TyKind::Named { args, .. } => {
                let (def, _) = user_adt(env, ty)?;
                self.build_user_adt(env, ty, def, args, label)
            }
            _ => None,
        }
    }

    fn build_user_adt(
        &mut self,
        env: &Env<'_, 'db>,
        ty: Ty<'db>,
        def: DefId<'db>,
        args: &[Ty<'db>],
        label: String,
    ) -> Option<TypeDesc> {
        let db = env.db;
        if let [key, value] = args
            && is_canonical_std_def_named(db, def, "mapping")
        {
            return Some(TypeDesc::Mapping {
                label,
                key: self.describe(env, *key)?,
                value: self.describe(env, *value)?,
            });
        }
        if let [element] = args
            && (is_canonical_std_def_named(db, def, "array")
                || is_canonical_std_def_named(db, def, "DynArray"))
        {
            return Some(TypeDesc::Array {
                label,
                element: self.describe(env, *element)?,
            });
        }

        let view = adt_view(db, def, args)?;
        let definition = Some(definition(env, view.adt));
        match view.ctors.as_slice() {
            [] => Some(TypeDesc::Variant {
                label: Some(label),
                definition,
                cases: Vec::new(),
            }),
            [ctor] => Some(TypeDesc::Struct {
                label: Some(label),
                definition,
                members: self.members(env, &ctor.fields, ctor.field_names.as_deref())?,
            }),
            // The compiler lowers constructor `n >= 2` as a tag of 1 around a
            // nested sum (`lower_injection`), so no stored value reads as the
            // declaration index. A case number is true only for two constructors.
            ctors if ctors.len() > 2 => None,
            ctors => {
                let cases = ctors
                    .iter()
                    .enumerate()
                    .map(|(index, ctor)| {
                        let payload = match ctor.fields.as_slice() {
                            [] => None,
                            [field] => Some(self.describe(env, *field)?),
                            _ => Some(self.entry(env, Key::Payload(ty, index as u32))?),
                        };
                        Some(Case {
                            name: ctor.name.clone(),
                            discriminant: index as u64,
                            payload,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(TypeDesc::Variant {
                    label: Some(label),
                    definition,
                    cases,
                })
            }
        }
    }
}

/// Which description a type gets, without describing its parts.
fn shape<'db>(env: &Env<'_, 'db>, ty: Ty<'db>) -> Shape {
    let db = env.db;
    let label = || display_ty(db, ty);
    match ty.kind(db) {
        TyKind::Named { ctor, args } => match ctor {
            TyCtor::Builtin(BuiltinTyCtor::Word) if args.is_empty() => {
                Shape::Elementary(TypeDesc::Bytes { label: label() })
            }
            TyCtor::Builtin(BuiltinTyCtor::Bool) if args.is_empty() => {
                Shape::Elementary(TypeDesc::Bool { label: label() })
            }
            TyCtor::Builtin(BuiltinTyCtor::Unit) if args.is_empty() => Shape::Complex,
            TyCtor::Builtin(BuiltinTyCtor::Sum) if args.len() == 2 => Shape::Complex,
            // The type of a string literal, the compile-time integer and the
            // binary product are not types a program value has at run time.
            TyCtor::Builtin(_) => Shape::Undescribable,
            TyCtor::User(user) if user.kind == UserTyCtorKind::Adt => {
                match std_leaf(env, user.def, args, label) {
                    Some(desc) => Shape::Elementary(desc),
                    None => Shape::Complex,
                }
            }
            // Contract types, and aliases that normalization left standing.
            TyCtor::User(_) => Shape::Undescribable,
        },
        TyKind::Tuple(_) | TyKind::Function { .. } => Shape::Complex,
        TyKind::Error | TyKind::Unknown | TyKind::BoundVar(_) | TyKind::Comptime(_) => {
            Shape::Undescribable
        }
    }
}

/// The std types whose meaning is fixed, each one word in std:
/// `uint256` (`std/std.sol`), `uint8` to `uint128` (`std/unsigned.sol`),
/// `address`, `bytes32`, and the `string` and `bytes` that a reference such as
/// `memory<string>` points to. A `uint` is described by its whole word and is
/// never truncated to its width.
fn std_leaf<'db>(
    env: &Env<'_, 'db>,
    def: DefId<'db>,
    args: &[Ty<'db>],
    label: impl FnOnce() -> String,
) -> Option<TypeDesc> {
    if !args.is_empty() {
        return None;
    }
    let db = env.db;
    let is = |name| is_canonical_std_def_named(db, def, name);
    if is("uint256")
        || UNSIGNED
            .iter()
            .any(|name| is_canonical_std_module_def_named(db, def, "unsigned", name))
    {
        Some(TypeDesc::Number { label: label() })
    } else if is("address") {
        Some(TypeDesc::Address { label: label() })
    } else if is("bytes32") || is("bytes") {
        Some(TypeDesc::Bytes { label: label() })
    } else if is("string") {
        Some(TypeDesc::String { label: label() })
    } else {
        None
    }
}

fn is_unit<'db>(db: &'db dyn hir_ty::Db, ty: Ty<'db>) -> bool {
    match ty.kind(db) {
        TyKind::Tuple(elems) => elems.is_empty(),
        TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Unit),
            args,
        } => args.is_empty(),
        _ => false,
    }
}

/// The definition and arguments of a user data type.
fn user_adt<'db>(env: &Env<'_, 'db>, ty: Ty<'db>) -> Option<(DefId<'db>, &'db [Ty<'db>])> {
    match ty.kind(env.db) {
        TyKind::Named {
            ctor: TyCtor::User(user),
            args,
        } if user.kind == UserTyCtorKind::Adt => Some((user.def, args)),
        _ => None,
    }
}

/// The source id, offset and length of `span`, when its file is one of the
/// program's sources.
pub(super) fn locate<'db>(env: &Env<'_, 'db>, span: Span<'db>) -> Option<(u32, u32, u32)> {
    let db = env.db;
    let span = span.resolve_to_absolute(db);
    let start = span.start.as_u32();
    let source = env.sources.id(span.file.url(db).as_str())?;
    Some((source, start, span.end.as_u32() - start))
}

/// The name of a type definition and where its source is.
fn definition<'db>(env: &Env<'_, 'db>, adt: AdtDef<'db>) -> Definition {
    let db = env.db;
    Definition {
        name: ident_text(db, &adt.name_elem(db)),
        location: locate(env, adt.span(db)),
    }
}

/// A data type with its type arguments filled in.
struct AdtView<'db> {
    adt: AdtDef<'db>,
    ctors: Vec<CtorView<'db>>,
}

struct CtorView<'db> {
    name: String,
    /// Written only when the source names every field, which a `struct` does.
    field_names: Option<Vec<String>>,
    fields: Vec<Ty<'db>>,
}

/// The constructors of `def` at `args`, from the representation plan that
/// Hull's layout uses, so that both agree on what a constructor's fields are.
fn adt_view<'db>(
    db: &'db dyn hir_ty::Db,
    def: DefId<'db>,
    args: &[Ty<'db>],
) -> Option<AdtView<'db>> {
    let module = parse_file_to_hir(db, def.file(db)).module(db);
    let adt = find_adt(db, module, def)?;
    if adt.ty_param_elems(db).len() != args.len() {
        return None;
    }
    let source = adt.ctors(db);
    if source.is_empty() {
        return Some(AdtView {
            adt,
            ctors: Vec::new(),
        });
    }
    let plan = hir_ty::derived_generic_plan(db, module, adt)?;
    let ctors = plan
        .from_arms
        .iter()
        .map(|arm| {
            let product = subst(db, arm.product_rep, args);
            let fields = split_product(db, product, arm.field_count as usize)?;
            let field_names = source
                .get(arm.ctor_index as usize)?
                .field_names
                .as_ref()
                .map(|names| {
                    names
                        .iter()
                        .map(|name| ident_text(db, name))
                        .collect::<Vec<String>>()
                })
                .filter(|names| names.len() == fields.len());
            Some(CtorView {
                name: arm.ctor_name.clone(),
                field_names,
                fields,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(AdtView { adt, ctors })
}

fn find_adt<'db>(
    db: &'db dyn hir_ty::Db,
    module: Module<'db>,
    def: DefId<'db>,
) -> Option<AdtDef<'db>> {
    module.items(db).iter().find_map(|item| match item {
        Item::AdtDef(adt) if adt.def_id_value(db) == def => Some(*adt),
        Item::ContractDef(contract) => contract.items(db).iter().find_map(|item| match item {
            ContractItem::AdtDef(adt) if adt.def_id_value(db) == def => Some(*adt),
            _ => None,
        }),
        _ => None,
    })
}

/// Replaces bound variable `n` by `args[n]`; a variable without an argument stays.
fn subst<'db>(db: &'db dyn hir_ty::Db, ty: Ty<'db>, args: &[Ty<'db>]) -> Ty<'db> {
    match ty.kind(db) {
        TyKind::BoundVar(var) => args.get(var.index as usize).copied().unwrap_or(ty),
        TyKind::Named { ctor, args: inner } => Ty::named(
            db,
            *ctor,
            inner.iter().map(|arg| subst(db, *arg, args)).collect(),
        ),
        TyKind::Function { params, ret } => Ty::function(
            db,
            params.iter().map(|param| subst(db, *param, args)).collect(),
            subst(db, *ret, args),
        ),
        TyKind::Tuple(elems) => Ty::tuple(
            db,
            elems.iter().map(|elem| subst(db, *elem, args)).collect(),
        ),
        TyKind::Comptime(inner) => Ty::comptime(db, subst(db, *inner, args)),
        TyKind::Error | TyKind::Unknown => ty,
    }
}

/// The `arity` fields of a constructor's product: none for unit, the type
/// itself for one field, and for more the left sides of a right-nested pair.
fn split_product<'db>(
    db: &'db dyn hir_ty::Db,
    mut product: Ty<'db>,
    arity: usize,
) -> Option<Vec<Ty<'db>>> {
    if arity == 0 {
        return is_unit(db, product).then(Vec::new);
    }
    let mut fields = Vec::with_capacity(arity);
    for _ in 1..arity {
        let TyKind::Named {
            ctor: TyCtor::Builtin(BuiltinTyCtor::Pair),
            args,
        } = product.kind(db)
        else {
            return None;
        };
        let [head, tail] = args.as_slice() else {
            return None;
        };
        fields.push(*head);
        product = *tail;
    }
    fields.push(product);
    Some(fields)
}

#[cfg(test)]
mod tests {
    use hir_ty::UserTyCtor;
    use serde_json::{Value, json};
    use solcore_test_utils::run_in_large_stack;

    use super::{super::test_support::lower, *};

    fn json_of(
        table: &mut TypeTable<'static>,
        env: &Env<'_, 'static>,
        ty: Ty<'static>,
    ) -> Option<Value> {
        table.describe(env, ty).map(|ty| ty.to_json())
    }

    /// `value` with every `{"id": ...}` replaced by the entry it names.
    fn resolved(table: &TypeTable<'_>, value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                if let (1, Some(Value::String(id))) = (object.len(), object.get("id")) {
                    let n: usize = id[1..].parse().unwrap();
                    return resolved(table, &table.descs[n].to_json());
                }
                Value::Object(
                    object
                        .iter()
                        .map(|(key, value)| (key.clone(), resolved(table, value)))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(|v| resolved(table, v)).collect()),
            other => other.clone(),
        }
    }

    #[test]
    fn types_follow_the_mapping() {
        run_in_large_stack(|| {
            let (db, module, program) = lower();
            let sources = SourceTable::from_program(db, &program);
            let env = Env {
                db,
                sources: &sources,
            };
            let field = |name: &str| {
                let facts = &program.debug.contracts["Types"];
                facts.iter().find(|field| field.name == name).unwrap().ty
            };
            // A data type declared in the fixture, at the given arguments.
            let adt = |name: &str, args: Vec<Ty<'static>>| {
                let def = module
                    .items(db)
                    .iter()
                    .find_map(|item| match item {
                        Item::AdtDef(adt) if ident_text(db, &adt.name_elem(db)) == name => {
                            Some(adt.def_id_value(db))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("no data type `{name}`"));
                let user = UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                };
                Ty::named(db, TyCtor::User(user), args)
            };
            let (word, bool_) = (Ty::word(db), Ty::bool(db));
            let mut table = TypeTable::default();
            let number = |label: &str| json!({"kind": "number", "label": label});

            // Elementary types are written in place.
            assert_eq!(
                json_of(&mut table, &env, field("small")),
                Some(number("uint8"))
            );
            assert_eq!(
                json_of(&mut table, &env, field("owner")),
                Some(json!({"kind": "address", "label": "address"}))
            );
            assert_eq!(
                json_of(&mut table, &env, field("hash")),
                Some(json!({"kind": "bytes", "label": "bytes32"}))
            );
            assert_eq!(
                json_of(&mut table, &env, field("flag")),
                Some(json!({"kind": "bool", "label": "bool"}))
            );
            assert_eq!(
                json_of(&mut table, &env, word),
                Some(json!({"kind": "bytes", "label": "word"}))
            );

            // Three constructors have no description, nor has a type that holds one:
            // the compiler stores the third as a tag of 1 around a nested sum, so
            // no stored value reads as the declaration index 2. Nothing is left in
            // the table.
            let shape = adt("Shape", vec![]);
            assert!(json_of(&mut table, &env, shape).is_none());
            assert!(json_of(&mut table, &env, adt("Pair", vec![shape, word])).is_none());
            assert!(json_of(&mut table, &env, Ty::tuple(db, vec![word, shape])).is_none());
            assert!(table.descriptions().is_empty());

            // Everything else is an entry of the table. The entries are looked up
            // by their labels: the identifiers follow the order of the walk, which is
            // not what is tested.
            let tuple = Ty::tuple(db, vec![word, bool_]);
            let pair = adt("Pair", vec![bool_, field("small")]);
            let list = adt("List", vec![word]);
            let ids = [
                tuple,
                pair,
                list,
                adt("Never", vec![]),
                adt("Point", vec![]),
            ]
            .map(|ty| json_of(&mut table, &env, ty).unwrap());
            let index_of = |label: &str| -> usize {
                let found: Vec<usize> = (0..table.descs.len())
                    .filter(|&n| table.descs[n].to_json()["label"] == label)
                    .collect();
                assert_eq!(found.len(), 1, "entries labelled `{label}`");
                found[0]
            };
            let labels = [
                "(word, bool)",
                "Pair<bool, uint8>",
                "List<word>",
                "Never",
                "Point",
            ];
            assert_eq!(
                ids,
                labels.map(|label| json!({ "id": format!("t{}", index_of(label)) }))
            );
            let named = |label: &str| resolved(&table, &table.descs[index_of(label)].to_json());
            assert_eq!(
                named("(word, bool)"),
                json!({"kind": "struct", "label": "(word, bool)", "contains": [
                    {"type": {"kind": "bytes", "label": "word"}},
                    {"type": {"kind": "bool", "label": "bool"}},
                ]})
            );
            assert_eq!(named("Pair<bool, uint8>")["definition"]["name"], "Pair");
            assert_eq!(
                named("Pair<bool, uint8>")["contains"],
                json!([
                    {"type": {"kind": "bool", "label": "bool"}},
                    {"type": number("uint8")},
                ])
            );
            // A recursive type names itself by id instead of expanding. It has two
            // constructors, so its cases are numbered 0 and 1.
            let list_id = format!("t{}", index_of("List<word>"));
            let list = table.descs[index_of("List<word>")].to_json();
            assert_eq!(list["cases"][0], json!({"name": "Nil", "discriminant": 0}));
            let payload = list["cases"][1]["payload"]["type"]["id"].as_str().unwrap();
            assert_eq!(
                list["cases"][1],
                json!({"name": "Cons", "discriminant": 1, "payload": {"type": {"id": payload}}})
            );
            assert_ne!(payload, list_id);
            assert_eq!(
                table.descs[payload[1..].parse::<usize>().unwrap()].to_json(),
                json!({"kind": "struct", "contains": [
                    {"type": {"kind": "bytes", "label": "word"}},
                    {"type": {"id": list_id}},
                ]})
            );
            // A type without constructors has no valid value.
            assert_eq!(named("Never")["cases"], json!([]));
            assert_eq!(named("Never")["definition"]["name"], "Never");
            // A struct names its members.
            assert_eq!(
                named("Point")["contains"],
                json!([
                    {"name": "x", "type": {"kind": "bytes", "label": "word"}},
                    {"name": "y", "type": {"kind": "bytes", "label": "word"}},
                ])
            );
            // A reference is a struct of one word, as std defines it.
            let text = table.describe(&env, field("text")).unwrap().to_json();
            let text = resolved(&table, &text);
            assert_eq!(text["label"], "memory<string>");
            assert_eq!(
                text["contains"],
                json!([{"type": {"kind": "bytes", "label": "word"}}])
            );
            let nested = table.describe(&env, field("nested")).unwrap().to_json();
            assert_eq!(
                resolved(&table, &nested),
                json!({"kind": "mapping", "label": "mapping(address => mapping(word => bool))",
                "contains": {
                    "key": {"type": {"kind": "address", "label": "address"}},
                    "value": {"type": {"kind": "mapping", "label": "mapping(word => bool)",
                        "contains": {
                            "key": {"type": {"kind": "bytes", "label": "word"}},
                            "value": {"type": {"kind": "bool", "label": "bool"}},
                        }}},
                }})
            );
            let words = table.describe(&env, field("words")).unwrap().to_json();
            assert_eq!(
                resolved(&table, &words),
                json!({"kind": "array", "label": "array<word>",
                    "contains": {"type": {"kind": "bytes", "label": "word"}}})
            );
        });
    }

    #[test]
    fn a_type_with_an_undescribable_part_has_no_description_and_leaves_no_trace() {
        run_in_large_stack(|| {
            let (db, module, program) = lower();
            let sources = SourceTable::from_program(db, &program);
            let env = Env {
                db,
                sources: &sources,
            };
            let def = module
                .items(db)
                .iter()
                .find_map(|item| match item {
                    Item::AdtDef(adt) if ident_text(db, &adt.name_elem(db)) == "Pair" => {
                        Some(adt.def_id_value(db))
                    }
                    _ => None,
                })
                .unwrap();
            let user = UserTyCtor {
                def,
                kind: UserTyCtorKind::Adt,
            };
            let around = |args| Ty::named(db, TyCtor::User(user), args);
            let word = Ty::word(db);
            let comptime = Ty::comptime(db, word);
            let mut table = TypeTable::default();
            for hopeless in [
                comptime,
                Ty::unknown(db),
                Ty::error(db),
                Ty::bound(db, 0),
                Ty::integer(db),
                Ty::string(db),
                Ty::tuple(db, vec![word, comptime]),
                Ty::tuple(db, vec![word, Ty::tuple(db, vec![comptime])]),
                around(vec![word, comptime]),
                // Too few arguments for the data type.
                around(vec![word]),
                Ty::function(db, vec![comptime], word),
            ] {
                assert!(
                    table.describe(&env, hopeless).is_none(),
                    "{}",
                    hopeless.display(db)
                );
                assert!(table.descriptions().is_empty(), "{}", hopeless.display(db));
            }
            // The first type described afterwards has the first id.
            assert_eq!(
                table
                    .describe(&env, around(vec![word, word]))
                    .map(|ty| ty.to_json()),
                Some(json!({"id": "t0"}))
            );
        });
    }
}

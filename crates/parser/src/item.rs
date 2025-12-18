//! Top-level item AST types for solcore.

use chumsky::{input::ValueInput, prelude::*};

use crate::{
    Ident, ParserErr, Pred, Signature, Span, Spanned, Type, ident_parser,
    lexer::Token,
    pred_parser, signature_parser,
    stmt::{Stmt, stmt_parser},
    type_parser,
};

/// Function definition: `function name(params) -> RetType { body }`.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef<'a> {
    pub sig: Spanned<Signature<'a>>,
    pub body: Vec<Spanned<Stmt<'a>>>,
}

/// Type alias: `type Name = Type`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAlias<'a> {
    pub name: Spanned<Ident<'a>>,
    pub ty: Spanned<Type<'a>>,
}

/// Data constructor: `Ctor` or `Ctor(Type, Type)`.
#[derive(Debug, Clone, PartialEq)]
pub struct DataCtor<'a> {
    pub name: Spanned<Ident<'a>>,
    pub fields: Vec<Spanned<Type<'a>>>,
}

/// ADT definition: `data Name(ty_params) = Ctor1 | Ctor2(Type)`.
#[derive(Debug, Clone, PartialEq)]
pub struct AdtDef<'a> {
    pub name: Spanned<Ident<'a>>,
    pub ty_params: Vec<Spanned<Ident<'a>>>,
    pub ctors: Vec<Spanned<DataCtor<'a>>>,
}

/// Class definition: `forall ty_vars. preds => class ty:ClassName(args) { methods }`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassDef<'a> {
    /// Type variables (from `forall a b.`).
    pub type_vars: Vec<Spanned<Ident<'a>>>,
    /// Superclass constraints (from `a : Eq =>`).
    pub super_preds: Vec<Spanned<Pred<'a>>>,
    /// Class head predicate (e.g., `a : Ord`).
    pub head: Spanned<Pred<'a>>,
    /// Method signatures (without bodies, just declarations).
    pub methods: Vec<Spanned<Signature<'a>>>,
}

/// Top-level item.
#[derive(Debug, Clone, PartialEq)]
pub enum Item<'a> {
    FunctionDef(FunctionDef<'a>),
    TypeAlias(TypeAlias<'a>),
    AdtDef(AdtDef<'a>),
    ClassDef(ClassDef<'a>),
}

/// Creates a parser for function definitions.
pub fn function_def_parser<'a, I>() -> impl Parser<'a, I, Spanned<FunctionDef<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    signature_parser()
        .then(
            stmt_parser()
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with(|(sig, body), e| (FunctionDef { sig, body }, e.span()))
        .boxed()
}

/// Creates a parser for type aliases: `type Name = Type;`.
pub fn type_alias_parser<'a, I>() -> impl Parser<'a, I, Spanned<TypeAlias<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    just(Token::Type)
        .ignore_then(ident_parser())
        .then_ignore(just(Token::Eq))
        .then(type_parser())
        .then_ignore(just(Token::Semi))
        .map_with(|(name, ty), e| (TypeAlias { name, ty }, e.span()))
        .boxed()
}

/// Creates a parser for data constructors: `Ctor` or `Ctor(Type, Type)`.
pub fn data_ctor_parser<'a, I>() -> impl Parser<'a, I, Spanned<DataCtor<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    let fields = type_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|f| f.unwrap_or_default());

    ident_parser()
        .then(fields)
        .map_with(|(name, fields), e| (DataCtor { name, fields }, e.span()))
        .boxed()
}

/// Creates a parser for ADT definitions: `data Name(ty_params) = Ctor1 | Ctor2(Type);`.
pub fn adt_def_parser<'a, I>() -> impl Parser<'a, I, Spanned<AdtDef<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    let ty_params = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|p| p.unwrap_or_default());

    let ctors = just(Token::Eq)
        .ignore_then(
            data_ctor_parser()
                .separated_by(just(Token::Pipe))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .or_not()
        .map(|c| c.unwrap_or_default());

    just(Token::Data)
        .ignore_then(ident_parser())
        .then(ty_params)
        .then(ctors)
        .then_ignore(just(Token::Semi))
        .map_with(|((name, ty_params), ctors), e| {
            (
                AdtDef {
                    name,
                    ty_params,
                    ctors,
                },
                e.span(),
            )
        })
        .boxed()
}

/// Creates a parser for method signatures inside class definitions.
/// Syntax: `function name(params) -> RetType;` (without body).
fn method_sig_parser<'a, I>() -> impl Parser<'a, I, Spanned<Signature<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    signature_parser()
        .then_ignore(just(Token::Semi))
        .boxed()
}

/// Creates a parser for class definitions.
/// Syntax: `forall ty_vars. preds => class ty:ClassName(args) { methods }`.
pub fn class_def_parser<'a, I>() -> impl Parser<'a, I, Spanned<ClassDef<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    // Type variables: `forall a b.` (whitespace-separated)
    let type_vars = just(Token::Forall)
        .ignore_then(ident_parser().repeated().collect::<Vec<_>>())
        .then_ignore(just(Token::Dot))
        .or_not()
        .map(|vars| vars.unwrap_or_default())
        .boxed();

    // Superclass predicates: `pred1, pred2 =>`
    let super_preds = pred_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .then_ignore(just(Token::FatArrow))
        .or_not()
        .map(|preds| preds.unwrap_or_default())
        .boxed();

    // Method signatures inside braces
    let methods = method_sig_parser()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .boxed();

    type_vars
        .then(super_preds)
        .then_ignore(just(Token::Class))
        .then(pred_parser())
        .then(methods)
        .map_with(|(((type_vars, super_preds), head), methods), e| {
            (
                ClassDef {
                    type_vars,
                    super_preds,
                    head,
                    methods,
                },
                e.span(),
            )
        })
        .boxed()
}

/// Creates a parser for top-level items.
pub fn item_parser<'a, I>() -> impl Parser<'a, I, Spanned<Item<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    let function_def = function_def_parser()
        .map_with(|(def, _), e| (Item::FunctionDef(def), e.span()))
        .boxed();

    let type_alias = type_alias_parser()
        .map_with(|(alias, _), e| (Item::TypeAlias(alias), e.span()))
        .boxed();

    let adt_def = adt_def_parser()
        .map_with(|(d, _), e| (Item::AdtDef(d), e.span()))
        .boxed();

    let class_def = class_def_parser()
        .map_with(|(d, _), e| (Item::ClassDef(d), e.span()))
        .boxed();

    choice((function_def, type_alias, adt_def, class_def))
}

#[cfg(test)]
mod tests {
    use chumsky::input::Stream;
    use logos::Logos;

    use super::*;

    fn make_stream(src: &str) -> impl ValueInput<'_, Token = Token<'_>, Span = Span> {
        let token_iter = Token::lexer(src).spanned().map(|(tok, span)| match tok {
            Ok(tok) => (tok, Span::from(span)),
            Err(()) => panic!("Unexpected lexer error"),
        });
        Stream::from_iter(token_iter).map((0..src.len()).into(), |(t, s): (_, _)| (t, s))
    }

    #[test]
    fn test_function_def_empty() {
        let result = function_def_parser().parse(make_stream("function foo() {}"));
        let (def, _) = result.into_result().unwrap();
        assert_eq!(def.sig.0.name.0, Ident("foo"));
        assert!(def.body.is_empty());
    }

    #[test]
    fn test_function_def_with_body() {
        let result = function_def_parser().parse(make_stream("function foo() { return 42; }"));
        let (def, _) = result.into_result().unwrap();
        assert_eq!(def.sig.0.name.0, Ident("foo"));
        assert_eq!(def.body.len(), 1);
    }

    #[test]
    fn test_function_def_with_params_and_return() {
        let result = function_def_parser().parse(make_stream(
            "function add(x : Int, y : Int) -> Int { return x; }",
        ));
        let (def, _) = result.into_result().unwrap();
        assert_eq!(def.sig.0.name.0, Ident("add"));
        assert_eq!(def.sig.0.params.len(), 2);
        assert!(def.sig.0.ret.is_some());
        assert_eq!(def.body.len(), 1);
    }

    #[test]
    fn test_function_def_with_forall() {
        let result = function_def_parser().parse(make_stream(
            "forall a. a : Eq => function eq(x : a, y : a) -> Bool { return x; }",
        ));
        let (def, _) = result.into_result().unwrap();
        assert_eq!(def.sig.0.type_vars.len(), 1);
        assert_eq!(def.sig.0.preds.len(), 1);
        assert_eq!(def.sig.0.name.0, Ident("eq"));
        assert_eq!(def.body.len(), 1);
    }

    #[test]
    fn test_type_alias_simple() {
        let result = type_alias_parser().parse(make_stream("type Address = word;"));
        let (alias, _) = result.into_result().unwrap();
        assert_eq!(alias.name.0, Ident("Address"));
        assert!(matches!(&alias.ty.0, Type::Named { name, .. } if name.0 == Ident("word")));
    }

    #[test]
    fn test_type_alias_with_args() {
        let result =
            type_alias_parser().parse(make_stream("type Balance = mapping(Address, word);"));
        let (alias, _) = result.into_result().unwrap();
        assert_eq!(alias.name.0, Ident("Balance"));
        match &alias.ty.0 {
            Type::Named { name, args } => {
                assert_eq!(name.0, Ident("mapping"));
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected Named type"),
        }
    }

    #[test]
    fn test_type_alias_fn_type() {
        let result = type_alias_parser().parse(make_stream("type Handler = (word) -> word;"));
        let (alias, _) = result.into_result().unwrap();
        assert_eq!(alias.name.0, Ident("Handler"));
        assert!(matches!(&alias.ty.0, Type::Fn { .. }));
    }

    #[test]
    fn test_type_alias_tuple() {
        let result = type_alias_parser().parse(make_stream("type Pair = (word, word);"));
        let (alias, _) = result.into_result().unwrap();
        assert_eq!(alias.name.0, Ident("Pair"));
        match &alias.ty.0 {
            Type::Tuple { elems } => assert_eq!(elems.len(), 2),
            _ => panic!("Expected Tuple type"),
        }
    }

    #[test]
    fn test_adt_def_single_nullary() {
        let result = adt_def_parser().parse(make_stream("data Unit = Unit;"));
        let (adt, _) = result.into_result().unwrap();
        assert_eq!(adt.name.0, Ident("Unit"));
        assert!(adt.ty_params.is_empty());
        assert_eq!(adt.ctors.len(), 1);
        assert_eq!(adt.ctors[0].0.name.0, Ident("Unit"));
        assert!(adt.ctors[0].0.fields.is_empty());
    }

    #[test]
    fn test_adt_def_single_with_fields() {
        let result = adt_def_parser().parse(make_stream("data Pair = Pair(word, word);"));
        let (adt, _) = result.into_result().unwrap();
        assert_eq!(adt.name.0, Ident("Pair"));
        assert!(adt.ty_params.is_empty());
        assert_eq!(adt.ctors.len(), 1);
        assert_eq!(adt.ctors[0].0.fields.len(), 2);
    }

    #[test]
    fn test_adt_def_multiple_ctors() {
        let result = adt_def_parser().parse(make_stream("data Bool = True | False;"));
        let (adt, _) = result.into_result().unwrap();
        assert_eq!(adt.name.0, Ident("Bool"));
        assert!(adt.ty_params.is_empty());
        assert_eq!(adt.ctors.len(), 2);
        assert_eq!(adt.ctors[0].0.name.0, Ident("True"));
        assert_eq!(adt.ctors[1].0.name.0, Ident("False"));
    }

    #[test]
    fn test_adt_def_with_ty_params() {
        let result = adt_def_parser().parse(make_stream("data Option(t) = None | Some(t);"));
        let (adt, _) = result.into_result().unwrap();
        assert_eq!(adt.name.0, Ident("Option"));
        assert_eq!(adt.ty_params.len(), 1);
        assert_eq!(adt.ty_params[0].0, Ident("t"));
        assert_eq!(adt.ctors.len(), 2);
        assert_eq!(adt.ctors[0].0.name.0, Ident("None"));
        assert_eq!(adt.ctors[1].0.name.0, Ident("Some"));
    }

    #[test]
    fn test_adt_def_multi_ty_params() {
        let result = adt_def_parser().parse(make_stream("data Either(a, b) = Left(a) | Right(b);"));
        let (adt, _) = result.into_result().unwrap();
        assert_eq!(adt.name.0, Ident("Either"));
        assert_eq!(adt.ty_params.len(), 2);
        assert_eq!(adt.ty_params[0].0, Ident("a"));
        assert_eq!(adt.ty_params[1].0, Ident("b"));
        assert_eq!(adt.ctors.len(), 2);
    }

    #[test]
    fn test_adt_def_void() {
        let result = adt_def_parser().parse(make_stream("data Void;"));
        let (adt, _) = result.into_result().unwrap();
        assert_eq!(adt.name.0, Ident("Void"));
        assert!(adt.ty_params.is_empty());
        assert!(adt.ctors.is_empty());
    }

    #[test]
    fn test_class_def_simple() {
        let result = class_def_parser().parse(make_stream("class a : Eq {}"));
        let (class, _) = result.into_result().unwrap();
        assert!(class.type_vars.is_empty());
        assert!(class.super_preds.is_empty());
        assert_eq!(class.head.0.class.0, Ident("Eq"));
        assert!(class.methods.is_empty());
    }

    #[test]
    fn test_class_def_with_forall() {
        let result = class_def_parser().parse(make_stream(
            "forall self fieldType offsetType. class self : StructField(fieldType, offsetType) {}",
        ));
        let (class, _) = result.into_result().unwrap();
        assert_eq!(class.type_vars.len(), 3);
        assert_eq!(class.type_vars[0].0, Ident("self"));
        assert_eq!(class.type_vars[1].0, Ident("fieldType"));
        assert_eq!(class.type_vars[2].0, Ident("offsetType"));
        assert!(class.super_preds.is_empty());
        assert_eq!(class.head.0.class.0, Ident("StructField"));
        assert_eq!(class.head.0.args.len(), 2);
        assert!(class.methods.is_empty());
    }

    #[test]
    fn test_class_def_with_super_pred() {
        let result = class_def_parser().parse(make_stream("forall a. a : Eq => class a : Ord {}"));
        let (class, _) = result.into_result().unwrap();
        assert_eq!(class.type_vars.len(), 1);
        assert_eq!(class.type_vars[0].0, Ident("a"));
        assert_eq!(class.super_preds.len(), 1);
        assert_eq!(class.super_preds[0].0.class.0, Ident("Eq"));
        assert_eq!(class.head.0.class.0, Ident("Ord"));
        assert!(class.methods.is_empty());
    }

    #[test]
    fn test_class_def_with_methods() {
        let result = class_def_parser().parse(make_stream(
            "forall a. a : Eq => class a : Ord { function gt(x : a, y : a) -> bool; }",
        ));
        let (class, _) = result.into_result().unwrap();
        assert_eq!(class.type_vars.len(), 1);
        assert_eq!(class.super_preds.len(), 1);
        assert_eq!(class.head.0.class.0, Ident("Ord"));
        assert_eq!(class.methods.len(), 1);
        assert_eq!(class.methods[0].0.name.0, Ident("gt"));
        assert_eq!(class.methods[0].0.params.len(), 2);
    }

    #[test]
    fn test_class_def_multiple_methods() {
        let result = class_def_parser().parse(make_stream(
            "forall a. class a : Eq { function eq(x : a, y : a) -> bool; function ne(x : a, y : a) -> bool; }",
        ));
        let (class, _) = result.into_result().unwrap();
        assert_eq!(class.type_vars.len(), 1);
        assert!(class.super_preds.is_empty());
        assert_eq!(class.head.0.class.0, Ident("Eq"));
        assert_eq!(class.methods.len(), 2);
        assert_eq!(class.methods[0].0.name.0, Ident("eq"));
        assert_eq!(class.methods[1].0.name.0, Ident("ne"));
    }
}

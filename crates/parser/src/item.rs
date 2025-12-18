//! Top-level item AST types for solcore.

use chumsky::{input::ValueInput, prelude::*};

use crate::{
    Ident, ParserErr, Signature, Span, Spanned, Type, ident_parser,
    lexer::Token,
    signature_parser,
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

/// Top-level item.
#[derive(Debug, Clone, PartialEq)]
pub enum Item<'a> {
    FunctionDef(FunctionDef<'a>),
    TypeAlias(TypeAlias<'a>),
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

/// Creates a parser for type aliases: `type Name = Type`.
pub fn type_alias_parser<'a, I>() -> impl Parser<'a, I, Spanned<TypeAlias<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    just(Token::Type)
        .ignore_then(ident_parser())
        .then_ignore(just(Token::Eq))
        .then(type_parser())
        .map_with(|(name, ty), e| (TypeAlias { name, ty }, e.span()))
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

    choice((function_def, type_alias))
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
        let result = type_alias_parser().parse(make_stream("type Address = word"));
        let (alias, _) = result.into_result().unwrap();
        assert_eq!(alias.name.0, Ident("Address"));
        assert!(matches!(&alias.ty.0, Type::Named { name, .. } if name.0 == Ident("word")));
    }

    #[test]
    fn test_type_alias_with_args() {
        let result = type_alias_parser().parse(make_stream("type Balance = mapping(Address, word)"));
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
        let result = type_alias_parser().parse(make_stream("type Handler = (word) -> word"));
        let (alias, _) = result.into_result().unwrap();
        assert_eq!(alias.name.0, Ident("Handler"));
        assert!(matches!(&alias.ty.0, Type::Fn { .. }));
    }

    #[test]
    fn test_type_alias_tuple() {
        let result = type_alias_parser().parse(make_stream("type Pair = (word, word)"));
        let (alias, _) = result.into_result().unwrap();
        assert_eq!(alias.name.0, Ident("Pair"));
        match &alias.ty.0 {
            Type::Tuple { elems } => assert_eq!(elems.len(), 2),
            _ => panic!("Expected Tuple type"),
        }
    }
}

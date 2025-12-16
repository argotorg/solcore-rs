pub mod expr;
pub mod lexer;

use chumsky::{input::ValueInput, prelude::*, span::SimpleSpan};

use lexer::Token;

pub use expr::{expr_parser, BinOp, Expr};

/// Span type.
pub type Span = SimpleSpan;

/// Spanned wrapper for AST nodes.
pub type Spanned<T> = (T, Span);

/// Parser extra type with simple error and no context.
pub type ParserErr<'a> = extra::Err<Simple<'a, Token<'a>>>;

/// Identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct Ident<'a>(pub &'a str);

/// Literal values.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit<'a> {
    /// Decimal number literal.
    Number(&'a str),
    /// Hexadecimal literal. The string include the leading `0x`.
    Hex(&'a str),
    /// String literal.
    String(&'a str),
}

/// Type expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Type<'a> {
    /// Named type with optional type arguments (e.g., `uint256`, `mapping(address, uint256)`).
    Named {
        name: Spanned<Ident<'a>>,
        args: Vec<Spanned<Type<'a>>>,
    },
    /// Function type (e.g., `(word, word) -> word`).
    Fn {
        params: Vec<Spanned<Type<'a>>>,
        ret: Box<Spanned<Type<'a>>>,
    },
    /// Tuple type (e.g., `()`, `(a, b)`).
    Tuple { elems: Vec<Spanned<Type<'a>>> },
}

/// Predicate/constraint (e.g., `a : Eq`, `a : Foo(b, c)`).
#[derive(Debug, Clone, PartialEq)]
pub struct Pred<'a> {
    /// The type being constrained (e.g., `a` in `a : Eq`).
    pub ty: Spanned<Type<'a>>,
    /// The class/constraint name (e.g., `Eq` in `a : Eq`).
    pub class: Spanned<Ident<'a>>,
    /// Type arguments to the class (e.g., `[b, c]` in `a : Foo(b, c)`).
    pub args: Vec<Spanned<Type<'a>>>,
}

/// Creates a parser for identifiers.
pub fn ident_parser<'a, I>() -> impl Parser<'a, I, Spanned<Ident<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    select! {
        Token::Ident(name) => Ident(name),
    }
    .map_with(|ident, e| (ident, e.span()))
}

/// Creates a parser for literals.
pub fn lit_parser<'a, I>() -> impl Parser<'a, I, Spanned<Lit<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    select! {
        Token::Number(n) => Lit::Number(n),
        Token::HexLit(h) => Lit::Hex(h),
        Token::String(s) => Lit::String(s),
    }
    .map_with(|lit, e| (lit, e.span()))
}

/// Creates a parser for types.
pub fn type_parser<'a, I>() -> impl Parser<'a, I, Spanned<Type<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    recursive(|ty| {
        // Named type: `ident` optionally followed by `(type_args)`.
        let type_args = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .or_not()
            .map(|args| args.unwrap_or_default())
            .boxed();

        let named_type = ident_parser()
            .then(type_args)
            .map_with(|(name, args), e| (Type::Named { name, args }, e.span()))
            .boxed();

        // Parenthesized types: function type or tuple type.
        let paren_types = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .boxed();

        // Function type: `(param_types) -> return_type`.
        let fn_type = paren_types
            .clone()
            .then_ignore(just(Token::Arrow))
            .then(ty)
            .map_with(|(params, ret), e| {
                (
                    Type::Fn {
                        params,
                        ret: Box::new(ret),
                    },
                    e.span(),
                )
            })
            .boxed();

        // Tuple type: `(types)` without `->`.
        let tuple_type = paren_types
            .map_with(|elems, e| (Type::Tuple { elems }, e.span()))
            .boxed();

        fn_type.or(tuple_type).or(named_type)
    })
}

/// Creates a parser for predicates/constraints (e.g., `a : Eq`, `a : Foo(b, c)`).
pub fn pred_parser<'a, I>() -> impl Parser<'a, I, Spanned<Pred<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    let class_args = type_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::LParen), just(Token::RParen))
        .or_not()
        .map(|args| args.unwrap_or_default())
        .boxed();

    type_parser()
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(class_args)
        .map_with(|((ty, class), args), e| (Pred { ty, class, args }, e.span()))
        .boxed()
}

#[cfg(test)]
mod tests {
    use chumsky::input::Stream;
    use logos::Logos;

    use super::*;

    /// Creates a token stream from source text.
    pub fn make_stream(src: &str) -> impl ValueInput<'_, Token = Token<'_>, Span = Span> {
        let token_iter = Token::lexer(src).spanned().map(|(tok, span)| match tok {
            Ok(tok) => (tok, Span::from(span)),
            Err(()) => panic!("Unexpected lexer error"),
        });
        Stream::from_iter(token_iter).map((0..src.len()).into(), |(t, s): (_, _)| (t, s))
    }

    #[test]
    fn test_parse_ident() {
        let result = ident_parser().parse(make_stream("foo"));
        assert_eq!(result.into_result(), Ok((Ident("foo"), Span::from(0..3))));
    }

    #[test]
    fn test_parse_number() {
        let result = lit_parser().parse(make_stream("42"));
        assert_eq!(
            result.into_result(),
            Ok((Lit::Number("42"), Span::from(0..2)))
        );
    }

    #[test]
    fn test_parse_hex() {
        let result = lit_parser().parse(make_stream("0xDEAD"));
        assert_eq!(
            result.into_result(),
            Ok((Lit::Hex("0xDEAD"), Span::from(0..6)))
        );
    }

    #[test]
    fn test_parse_string() {
        let result = lit_parser().parse(make_stream(r#""hello""#));
        assert_eq!(
            result.into_result(),
            Ok((Lit::String(r#""hello""#), Span::from(0..7)))
        );
    }

    #[test]
    fn test_parse_type_simple() {
        let result = type_parser().parse(make_stream("uint256"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Named { name, args } => {
                assert_eq!(name.0, Ident("uint256"));
                assert!(args.is_empty());
            }
            _ => panic!("Expected Named type"),
        }
    }

    #[test]
    fn test_parse_type_with_args() {
        let result = type_parser().parse(make_stream("mapping(address, uint256)"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Named { name, args } => {
                assert_eq!(name.0, Ident("mapping"));
                assert_eq!(args.len(), 2);
                assert!(matches!(&args[0].0, Type::Named { name, .. } if name.0 == Ident("address")));
                assert!(matches!(&args[1].0, Type::Named { name, .. } if name.0 == Ident("uint256")));
            }
            _ => panic!("Expected Named type"),
        }
    }

    #[test]
    fn test_parse_type_nested() {
        let result = type_parser().parse(make_stream("mapping(address, Array(uint256))"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Named { name, args } => {
                assert_eq!(name.0, Ident("mapping"));
                assert_eq!(args.len(), 2);
                match &args[1].0 {
                    Type::Named { name, args } => {
                        assert_eq!(name.0, Ident("Array"));
                        assert_eq!(args.len(), 1);
                    }
                    _ => panic!("Expected nested Named type"),
                }
            }
            _ => panic!("Expected Named type"),
        }
    }

    #[test]
    fn test_parse_fn_type_simple() {
        let result = type_parser().parse(make_stream("(word) -> word"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Fn { params, ret } => {
                assert_eq!(params.len(), 1);
                assert!(matches!(&params[0].0, Type::Named { name, .. } if name.0 == Ident("word")));
                assert!(matches!(&ret.0, Type::Named { name, .. } if name.0 == Ident("word")));
            }
            _ => panic!("Expected Fn type"),
        }
    }

    #[test]
    fn test_parse_fn_type_multiple_params() {
        let result = type_parser().parse(make_stream("(word, word) -> word"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Fn { params, ret } => {
                assert_eq!(params.len(), 2);
                assert!(matches!(&params[0].0, Type::Named { name, .. } if name.0 == Ident("word")));
                assert!(matches!(&params[1].0, Type::Named { name, .. } if name.0 == Ident("word")));
                assert!(matches!(&ret.0, Type::Named { name, .. } if name.0 == Ident("word")));
            }
            _ => panic!("Expected Fn type"),
        }
    }

    #[test]
    fn test_parse_fn_type_no_params() {
        let result = type_parser().parse(make_stream("() -> word"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Fn { params, ret } => {
                assert!(params.is_empty());
                assert!(matches!(&ret.0, Type::Named { name, .. } if name.0 == Ident("word")));
            }
            _ => panic!("Expected Fn type"),
        }
    }

    #[test]
    fn test_parse_fn_type_returning_fn() {
        // (a) -> (b) -> c parses as (a) -> ((b) -> c) due to right-associativity.
        let result = type_parser().parse(make_stream("(a) -> (b) -> c"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Fn { params, ret } => {
                assert_eq!(params.len(), 1);
                assert!(matches!(&ret.0, Type::Fn { .. }));
            }
            _ => panic!("Expected Fn type"),
        }
    }

    #[test]
    fn test_parse_tuple_empty() {
        let result = type_parser().parse(make_stream("()"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Tuple { elems } => {
                assert!(elems.is_empty());
            }
            _ => panic!("Expected Tuple type"),
        }
    }

    #[test]
    fn test_parse_tuple_single() {
        let result = type_parser().parse(make_stream("(a)"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Tuple { elems } => {
                assert_eq!(elems.len(), 1);
                assert!(matches!(&elems[0].0, Type::Named { name, .. } if name.0 == Ident("a")));
            }
            _ => panic!("Expected Tuple type"),
        }
    }

    #[test]
    fn test_parse_tuple_multiple() {
        let result = type_parser().parse(make_stream("(a, b, c)"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Tuple { elems } => {
                assert_eq!(elems.len(), 3);
                assert!(matches!(&elems[0].0, Type::Named { name, .. } if name.0 == Ident("a")));
                assert!(matches!(&elems[1].0, Type::Named { name, .. } if name.0 == Ident("b")));
                assert!(matches!(&elems[2].0, Type::Named { name, .. } if name.0 == Ident("c")));
            }
            _ => panic!("Expected Tuple type"),
        }
    }

    #[test]
    fn test_parse_tuple_nested() {
        let result = type_parser().parse(make_stream("((a, b), c)"));
        let (ty, _) = result.into_result().unwrap();
        match ty {
            Type::Tuple { elems } => {
                assert_eq!(elems.len(), 2);
                assert!(matches!(&elems[0].0, Type::Tuple { .. }));
                assert!(matches!(&elems[1].0, Type::Named { name, .. } if name.0 == Ident("c")));
            }
            _ => panic!("Expected Tuple type"),
        }
    }

    #[test]
    fn test_parse_pred_simple() {
        // a : Eq
        let result = pred_parser().parse(make_stream("a : Eq"));
        let (pred, _) = result.into_result().unwrap();
        assert!(matches!(&pred.ty.0, Type::Named { name, .. } if name.0 == Ident("a")));
        assert_eq!(pred.class.0, Ident("Eq"));
        assert!(pred.args.is_empty());
    }

    #[test]
    fn test_parse_pred_with_args() {
        // a : Foo(b, c)
        let result = pred_parser().parse(make_stream("a : Foo(b, c)"));
        let (pred, _) = result.into_result().unwrap();
        assert!(matches!(&pred.ty.0, Type::Named { name, .. } if name.0 == Ident("a")));
        assert_eq!(pred.class.0, Ident("Foo"));
        assert_eq!(pred.args.len(), 2);
        assert!(matches!(&pred.args[0].0, Type::Named { name, .. } if name.0 == Ident("b")));
        assert!(matches!(&pred.args[1].0, Type::Named { name, .. } if name.0 == Ident("c")));
    }

    #[test]
    fn test_parse_pred_complex_type() {
        // (a, b) : Eq
        let result = pred_parser().parse(make_stream("(a, b) : Eq"));
        let (pred, _) = result.into_result().unwrap();
        assert!(matches!(&pred.ty.0, Type::Tuple { .. }));
        assert_eq!(pred.class.0, Ident("Eq"));
        assert!(pred.args.is_empty());
    }
}
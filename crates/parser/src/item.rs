//! Top-level item AST types for solcore.

use chumsky::{input::ValueInput, prelude::*};

use crate::{
    Ident, ParserErr, Signature, Span, Spanned, Type, ident_parser,
    lexer::Token,
    signature_parser,
    stmt::{Stmt, stmt_parser},
};

/// Function definition: `function name(params) -> RetType { body }`.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef<'a> {
    pub sig: Spanned<Signature<'a>>,
    pub body: Vec<Spanned<Stmt<'a>>>,
}

/// Top-level item.
#[derive(Debug, Clone, PartialEq)]
pub enum Item<'a> {
    FunctionDef(FunctionDef<'a>),
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

/// Creates a parser for top-level items.
pub fn item_parser<'a, I>() -> impl Parser<'a, I, Spanned<Item<'a>>, ParserErr<'a>>
where
    I: ValueInput<'a, Token = Token<'a>, Span = Span>,
{
    let function_def = function_def_parser()
        .map_with(|(def, _), e| (Item::FunctionDef(def), e.span()))
        .boxed();

    function_def
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
}

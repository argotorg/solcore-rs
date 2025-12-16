use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\n\r\f]+")]
pub enum Token<'a> {
    // Keywords.
    #[token("contract")]
    Contract,
    #[token("import")]
    Import,
    #[token("let")]
    Let,
    #[token("data")]
    Data,
    #[token("class")]
    Class,
    #[token("forall")]
    Forall,
    #[token("instance")]
    Instance,
    #[token("if")]
    If,
    #[token("else")]
    Else,
    #[token("for")]
    For,
    #[token("switch")]
    Switch,
    #[token("type")]
    Type,
    #[token("case")]
    Case,
    #[token("default")]
    Default,
    #[token("match")]
    Match,
    #[token("function")]
    Function,
    #[token("constructor")]
    Constructor,
    #[token("return")]
    Return,
    #[token("leave")]
    Leave,
    #[token("continue")]
    Continue,
    #[token("break")]
    Break,
    #[token("lam")]
    Lam,
    #[token("assembly")]
    Assembly,
    #[token("pragma")]
    Pragma,
    #[token("then")]
    Then,

    // Multi-character operators (must be defined before single-character ones).
    #[token(":=")]
    ColonEq,
    #[token("->")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("==")]
    EqEq,
    #[token("!=")]
    NotEq,
    #[token(">=")]
    GreaterEq,
    #[token("<=")]
    LessEq,
    #[token("&&")]
    AndAnd,
    #[token("||")]
    OrOr,
    #[token("+=")]
    PlusEq,
    #[token("-=")]
    MinusEq,

    // Single-character operators.
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("!")]
    Bang,
    #[token("<")]
    Less,
    #[token(">")]
    Greater,
    #[token("=")]
    Eq,
    #[token("|")]
    Pipe,

    // Punctuation.
    #[token(".")]
    Dot,
    #[token(":")]
    Colon,
    #[token(";")]
    Semi,
    #[token(",")]
    Comma,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("_")]
    Underscore,

    // Literals.
    #[regex(r"0x[0-9a-fA-F]+", |lex| lex.slice())]
    HexLit(&'a str),

    #[regex(r"[0-9]+", |lex| lex.slice())]
    Number(&'a str),

    #[regex(r#""([^"\\]|\\.)*""#, |lex| lex.slice())]
    String(&'a str),

    // Identifier.
    #[regex(r"[a-zA-Z][a-zA-Z0-9_]*", |lex| lex.slice())]
    Ident(&'a str),

    // Comments (skipped).
    #[token("//", line_comment)]
    LineComment,

    #[token("/*", block_comment)]
    BlockComment,
}

/// Skips a line comment starting with `//` by consuming all characters until the next newline.
fn line_comment<'a>(lex: &mut logos::Lexer<'a, Token<'a>>) -> logos::Skip {
    let remainder = lex.remainder();
    let len = remainder.find('\n').unwrap_or(remainder.len());
    lex.bump(len);
    logos::Skip
}

/// Skips a block comment starting with `/*` by consuming all characters until the matching `*/`.
/// Supports nested block comments by tracking depth.
fn block_comment<'a>(lex: &mut logos::Lexer<'a, Token<'a>>) -> logos::Skip {
    let remainder = lex.remainder();
    let mut depth = 1;
    let mut chars = remainder.char_indices();

    while let Some((i, c)) = chars.next() {
        match c {
            '*' => {
                if let Some((_, '/')) = chars.next() {
                    depth -= 1;
                    if depth == 0 {
                        lex.bump(i + 2);
                        return logos::Skip;
                    }
                }
            }
            '/' => {
                if let Some((_, '*')) = chars.next() {
                    depth += 1;
                }
            }
            _ => {}
        }
    }

    // Unclosed comment, consume the rest.
    lex.bump(remainder.len());
    logos::Skip
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to collect all tokens from input.
    fn tokenize(input: &str) -> Vec<Token<'_>> {
        Token::lexer(input).filter_map(Result::ok).collect()
    }

    #[test]
    fn test_keywords() {
        assert_eq!(tokenize("contract"), vec![Token::Contract]);
        assert_eq!(tokenize("import"), vec![Token::Import]);
        assert_eq!(tokenize("let"), vec![Token::Let]);
        assert_eq!(tokenize("data"), vec![Token::Data]);
        assert_eq!(tokenize("class"), vec![Token::Class]);
        assert_eq!(tokenize("forall"), vec![Token::Forall]);
        assert_eq!(tokenize("instance"), vec![Token::Instance]);
        assert_eq!(tokenize("if"), vec![Token::If]);
        assert_eq!(tokenize("else"), vec![Token::Else]);
        assert_eq!(tokenize("for"), vec![Token::For]);
        assert_eq!(tokenize("switch"), vec![Token::Switch]);
        assert_eq!(tokenize("type"), vec![Token::Type]);
        assert_eq!(tokenize("case"), vec![Token::Case]);
        assert_eq!(tokenize("default"), vec![Token::Default]);
        assert_eq!(tokenize("match"), vec![Token::Match]);
        assert_eq!(tokenize("function"), vec![Token::Function]);
        assert_eq!(tokenize("constructor"), vec![Token::Constructor]);
        assert_eq!(tokenize("return"), vec![Token::Return]);
        assert_eq!(tokenize("leave"), vec![Token::Leave]);
        assert_eq!(tokenize("continue"), vec![Token::Continue]);
        assert_eq!(tokenize("break"), vec![Token::Break]);
        assert_eq!(tokenize("lam"), vec![Token::Lam]);
        assert_eq!(tokenize("assembly"), vec![Token::Assembly]);
        assert_eq!(tokenize("pragma"), vec![Token::Pragma]);
        assert_eq!(tokenize("then"), vec![Token::Then]);
    }

    #[test]
    fn test_multi_char_operators() {
        assert_eq!(tokenize(":="), vec![Token::ColonEq]);
        assert_eq!(tokenize("->"), vec![Token::Arrow]);
        assert_eq!(tokenize("=>"), vec![Token::FatArrow]);
        assert_eq!(tokenize("=="), vec![Token::EqEq]);
        assert_eq!(tokenize("!="), vec![Token::NotEq]);
        assert_eq!(tokenize(">="), vec![Token::GreaterEq]);
        assert_eq!(tokenize("<="), vec![Token::LessEq]);
        assert_eq!(tokenize("&&"), vec![Token::AndAnd]);
        assert_eq!(tokenize("||"), vec![Token::OrOr]);
        assert_eq!(tokenize("+="), vec![Token::PlusEq]);
        assert_eq!(tokenize("-="), vec![Token::MinusEq]);
    }

    #[test]
    fn test_single_char_operators() {
        assert_eq!(tokenize("+"), vec![Token::Plus]);
        assert_eq!(tokenize("-"), vec![Token::Minus]);
        assert_eq!(tokenize("*"), vec![Token::Star]);
        assert_eq!(tokenize("/"), vec![Token::Slash]);
        assert_eq!(tokenize("%"), vec![Token::Percent]);
        assert_eq!(tokenize("!"), vec![Token::Bang]);
        assert_eq!(tokenize("<"), vec![Token::Less]);
        assert_eq!(tokenize(">"), vec![Token::Greater]);
        assert_eq!(tokenize("="), vec![Token::Eq]);
        assert_eq!(tokenize("|"), vec![Token::Pipe]);
    }

    #[test]
    fn test_punctuation() {
        assert_eq!(tokenize("."), vec![Token::Dot]);
        assert_eq!(tokenize(":"), vec![Token::Colon]);
        assert_eq!(tokenize(";"), vec![Token::Semi]);
        assert_eq!(tokenize(","), vec![Token::Comma]);
        assert_eq!(tokenize("("), vec![Token::LParen]);
        assert_eq!(tokenize(")"), vec![Token::RParen]);
        assert_eq!(tokenize("{"), vec![Token::LBrace]);
        assert_eq!(tokenize("}"), vec![Token::RBrace]);
        assert_eq!(tokenize("["), vec![Token::LBracket]);
        assert_eq!(tokenize("]"), vec![Token::RBracket]);
        assert_eq!(tokenize("_"), vec![Token::Underscore]);
    }

    #[test]
    fn test_literals() {
        // Hex literals.
        assert_eq!(tokenize("0x0"), vec![Token::HexLit("0x0")]);
        assert_eq!(tokenize("0xDEAD"), vec![Token::HexLit("0xDEAD")]);
        assert_eq!(tokenize("0xdeadbeef"), vec![Token::HexLit("0xdeadbeef")]);
        assert_eq!(tokenize("0x123ABC"), vec![Token::HexLit("0x123ABC")]);

        // Number literals.
        assert_eq!(tokenize("0"), vec![Token::Number("0")]);
        assert_eq!(tokenize("42"), vec![Token::Number("42")]);
        assert_eq!(tokenize("123456789"), vec![Token::Number("123456789")]);

        // String literals.
        assert_eq!(tokenize(r#""""#), vec![Token::String(r#""""#)]);
        assert_eq!(tokenize(r#""hello""#), vec![Token::String(r#""hello""#)]);
        assert_eq!(
            tokenize(r#""hello world""#),
            vec![Token::String(r#""hello world""#)]
        );
        assert_eq!(
            tokenize(r#""escaped \"quote\"""#),
            vec![Token::String(r#""escaped \"quote\"""#)]
        );
        assert_eq!(
            tokenize(r#""newline\\n""#),
            vec![Token::String(r#""newline\\n""#)]
        );
    }

    #[test]
    fn test_identifiers() {
        assert_eq!(tokenize("x"), vec![Token::Ident("x")]);
        assert_eq!(tokenize("foo"), vec![Token::Ident("foo")]);
        assert_eq!(tokenize("FooBar"), vec![Token::Ident("FooBar")]);
        assert_eq!(tokenize("foo_bar"), vec![Token::Ident("foo_bar")]);
        assert_eq!(tokenize("foo123"), vec![Token::Ident("foo123")]);
        assert_eq!(tokenize("x1_y2_z3"), vec![Token::Ident("x1_y2_z3")]);
    }

    #[test]
    fn test_identifiers_vs_keywords() {
        // Keywords should not be parsed as identifiers.
        assert_eq!(tokenize("if"), vec![Token::If]);
        assert_eq!(tokenize("ifx"), vec![Token::Ident("ifx")]);
        assert_eq!(tokenize("xif"), vec![Token::Ident("xif")]);
        assert_eq!(tokenize("letx"), vec![Token::Ident("letx")]);
        assert_eq!(tokenize("returnValue"), vec![Token::Ident("returnValue")]);
    }

    #[test]
    fn test_line_comments() {
        assert_eq!(tokenize("// comment"), vec![]);
        assert_eq!(tokenize("// comment\n"), vec![]);
        assert_eq!(tokenize("x // comment"), vec![Token::Ident("x")]);
        assert_eq!(
            tokenize("x // comment\ny"),
            vec![Token::Ident("x"), Token::Ident("y")]
        );
    }

    #[test]
    fn test_block_comments() {
        assert_eq!(tokenize("/* comment */"), vec![]);
        assert_eq!(
            tokenize("x /* comment */ y"),
            vec![Token::Ident("x"), Token::Ident("y")]
        );
        assert_eq!(tokenize("/* multi\nline\ncomment */"), vec![]);
        assert_eq!(tokenize("/* multi\nline*\ncomment */"), vec![]);
    }

    #[test]
    fn test_nested_block_comments() {
        assert_eq!(tokenize("/* outer /* inner */ outer */"), vec![]);
        assert_eq!(
            tokenize("x /* a /* b */ c */ y"),
            vec![Token::Ident("x"), Token::Ident("y")]
        );
        assert_eq!(tokenize("/* /* /* deeply */ nested */ comments */"), vec![]);
    }

    #[test]
    fn test_whitespace_handling() {
        assert_eq!(tokenize("   x   "), vec![Token::Ident("x")]);
        assert_eq!(tokenize("x\ty"), vec![Token::Ident("x"), Token::Ident("y")]);
        assert_eq!(tokenize("x\ny"), vec![Token::Ident("x"), Token::Ident("y")]);
        assert_eq!(
            tokenize("x\r\ny"),
            vec![Token::Ident("x"), Token::Ident("y")]
        );
    }

    #[test]
    fn test_complex_expression() {
        assert_eq!(
            tokenize("let x = 42;"),
            vec![
                Token::Let,
                Token::Ident("x"),
                Token::Eq,
                Token::Number("42"),
                Token::Semi,
            ]
        );

        assert_eq!(
            tokenize("if x >= 0 && y <= 10"),
            vec![
                Token::If,
                Token::Ident("x"),
                Token::GreaterEq,
                Token::Number("0"),
                Token::AndAnd,
                Token::Ident("y"),
                Token::LessEq,
                Token::Number("10"),
            ]
        );

        assert_eq!(
            tokenize("function foo(a, b) -> c"),
            vec![
                Token::Function,
                Token::Ident("foo"),
                Token::LParen,
                Token::Ident("a"),
                Token::Comma,
                Token::Ident("b"),
                Token::RParen,
                Token::Arrow,
                Token::Ident("c"),
            ]
        );
    }

    #[test]
    fn test_contract_snippet() {
        let input = r#"
            contract Foo {
                function bar() -> u256 {
                    let x := 0x1234;
                    return x
                }
            }
        "#;

        let tokens = tokenize(input);
        assert_eq!(
            tokens,
            vec![
                Token::Contract,
                Token::Ident("Foo"),
                Token::LBrace,
                Token::Function,
                Token::Ident("bar"),
                Token::LParen,
                Token::RParen,
                Token::Arrow,
                Token::Ident("u256"),
                Token::LBrace,
                Token::Let,
                Token::Ident("x"),
                Token::ColonEq,
                Token::HexLit("0x1234"),
                Token::Semi,
                Token::Return,
                Token::Ident("x"),
                Token::RBrace,
                Token::RBrace,
            ]
        );
    }
}

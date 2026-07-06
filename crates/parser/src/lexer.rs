//! Lexical tokens for the Solcore parser.
//!
//! Logos produces token spans in absolute byte offsets over the input string.
//! Comments and whitespace are skipped; invalid characters are reported by the
//! parser's tokenization wrapper so the rest of the grammar can recover.

use logos::Logos;

/// Lexer error kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LexError {
    /// Generic invalid token.
    #[default]
    Invalid,
    /// A block comment reached end of file before its matching terminator.
    UnterminatedBlockComment,
}

/// Token recognized by the Solcore lexer.
///
/// Literal and identifier variants borrow slices from the input source. Token
/// ordering matters for overlapping operators: multi-character operators are
/// defined before their single-character prefixes.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\n\r\f]+")]
#[logos(error = LexError)]
pub enum Token<'a> {
    /// `contract`.
    #[token("contract")]
    Contract,
    /// `import`.
    #[token("import")]
    Import,
    /// `export`.
    #[token("export")]
    Export,
    /// `as`.
    #[token("as")]
    As,
    /// `let`.
    #[token("let")]
    Let,
    /// `data`.
    #[token("data")]
    Data,
    /// `class`.
    #[token("class")]
    Class,
    /// `forall`.
    #[token("forall")]
    Forall,
    /// `instance`.
    #[token("instance")]
    Instance,
    /// `if`.
    #[token("if")]
    If,
    /// `else`.
    #[token("else")]
    Else,
    /// `for`.
    #[token("for")]
    For,
    /// `switch`.
    #[token("switch")]
    Switch,
    /// `type`.
    #[token("type")]
    Type,
    /// `case`.
    #[token("case")]
    Case,
    /// `default`.
    #[token("default")]
    Default,
    /// `match`.
    #[token("match")]
    Match,
    /// `public`.
    #[token("public")]
    Public,
    /// `payable`.
    #[token("payable")]
    Payable,
    /// `function`.
    #[token("function")]
    Function,
    /// `constructor`.
    #[token("constructor")]
    Constructor,
    /// `fallback`.
    #[token("fallback")]
    Fallback,
    /// `return`.
    #[token("return")]
    Return,
    /// `leave`.
    #[token("leave")]
    Leave,
    /// `continue`.
    #[token("continue")]
    Continue,
    /// `break`.
    #[token("break")]
    Break,
    /// `lam`.
    #[token("lam")]
    Lam,
    /// `assembly`.
    #[token("assembly")]
    Assembly,
    /// `pragma`.
    #[token("pragma")]
    Pragma,
    /// `true`.
    #[token("true")]
    True,
    /// `false`.
    #[token("false")]
    False,

    /// `:=`.
    #[token(":=")]
    ColonEq,
    /// `->`.
    #[token("->")]
    Arrow,
    /// `=>`.
    #[token("=>")]
    FatArrow,
    /// `==`.
    #[token("==")]
    EqEq,
    /// `!=`.
    #[token("!=")]
    NotEq,
    /// `>=`.
    #[token(">=")]
    GreaterEq,
    /// `<=`.
    #[token("<=")]
    LessEq,
    /// `&&`.
    #[token("&&")]
    AndAnd,
    /// `||`.
    #[token("||")]
    OrOr,
    /// `+=`.
    #[token("+=")]
    PlusEq,
    /// `-=`.
    #[token("-=")]
    MinusEq,
    /// `^=`.
    #[token("^=")]
    CaretEq,
    /// `&=`.
    #[token("&=")]
    AmpEq,
    /// `|=`.
    #[token("|=")]
    PipeEq,
    /// `%=`.
    #[token("%=")]
    PercentEq,

    /// `+`.
    #[token("+")]
    Plus,
    /// `-`.
    #[token("-")]
    Minus,
    /// `*`.
    #[token("*")]
    Star,
    /// `/`.
    #[token("/")]
    Slash,
    /// `%`.
    #[token("%")]
    Percent,
    /// `!`.
    #[token("!")]
    Bang,
    /// `<`.
    #[token("<")]
    Less,
    /// `>`.
    #[token(">")]
    Greater,
    /// `=`.
    #[token("=")]
    Eq,
    /// `|`.
    #[token("|")]
    Pipe,
    /// `&`.
    #[token("&")]
    Amp,
    /// `^`.
    #[token("^")]
    Caret,
    /// `@`.
    #[token("@")]
    At,
    /// `?`.
    #[token("?")]
    Question,

    /// `.`.
    #[token(".")]
    Dot,
    /// `:`.
    #[token(":")]
    Colon,
    /// `;`.
    #[token(";")]
    Semi,
    /// `,`.
    #[token(",")]
    Comma,
    /// `(`.
    #[token("(")]
    LParen,
    /// `)`.
    #[token(")")]
    RParen,
    /// `{`.
    #[token("{")]
    LBrace,
    /// `}`.
    #[token("}")]
    RBrace,
    /// `[`.
    #[token("[")]
    LBracket,
    /// `]`.
    #[token("]")]
    RBracket,
    /// `_`.
    #[token("_")]
    Underscore,

    /// Hexadecimal literal text.
    #[regex(r"0x[0-9a-fA-F]+", |lex| lex.slice())]
    HexLit(&'a str),

    /// Decimal number literal text.
    #[regex(r"[0-9]+", |lex| lex.slice())]
    Number(&'a str),

    /// Quoted string literal text, including quotes and escapes.
    #[regex(r#""([^"\\]|\\.)*""#, |lex| lex.slice())]
    String(&'a str),

    /// Identifier or pragma-name text.
    ///
    /// The lexer accepts hyphens so pragma names such as
    /// `no-bounded-variable-condition` tokenize as one item. The parser rejects
    /// hyphenated text in normal identifier positions.
    #[regex(r"[a-zA-Z][a-zA-Z0-9_]*(-[a-zA-Z][a-zA-Z0-9_]*)*", |lex| lex.slice())]
    Ident(&'a str),

    /// Line comment skipped by the lexer.
    #[token("//", line_comment)]
    LineComment,

    /// Block comment skipped by the lexer.
    #[token("/*", block_comment)]
    BlockComment,
}

/// Skips a line comment starting with `//` by consuming all characters until
/// the next newline.
fn line_comment<'a>(lex: &mut logos::Lexer<'a, Token<'a>>) -> logos::Skip {
    let remainder = lex.remainder();
    let len = remainder.find('\n').unwrap_or(remainder.len());
    lex.bump(len);
    logos::Skip
}

/// Skips a block comment starting with `/*` by consuming all characters until
/// the matching `*/`. Supports nested block comments by tracking depth.
fn block_comment<'a>(lex: &mut logos::Lexer<'a, Token<'a>>) -> Result<logos::Skip, LexError> {
    let remainder = lex.remainder();
    let mut depth = 1;
    let bytes = remainder.as_bytes();
    let mut i = 0;

    while i + 1 < bytes.len() {
        match (bytes[i], bytes[i + 1]) {
            (b'/', b'*') => {
                depth += 1;
                i += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    lex.bump(i);
                    return Ok(logos::Skip);
                }
            }
            _ => i += 1,
        }
    }

    lex.bump(remainder.len());
    Err(LexError::UnterminatedBlockComment)
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
        assert_eq!(tokenize("export"), vec![Token::Export]);
        assert_eq!(tokenize("as"), vec![Token::As]);
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
        assert_eq!(tokenize("public"), vec![Token::Public]);
        assert_eq!(tokenize("payable"), vec![Token::Payable]);
        assert_eq!(tokenize("function"), vec![Token::Function]);
        assert_eq!(tokenize("constructor"), vec![Token::Constructor]);
        assert_eq!(tokenize("fallback"), vec![Token::Fallback]);
        assert_eq!(tokenize("return"), vec![Token::Return]);
        assert_eq!(tokenize("leave"), vec![Token::Leave]);
        assert_eq!(tokenize("continue"), vec![Token::Continue]);
        assert_eq!(tokenize("break"), vec![Token::Break]);
        assert_eq!(tokenize("lam"), vec![Token::Lam]);
        assert_eq!(tokenize("assembly"), vec![Token::Assembly]);
        assert_eq!(tokenize("pragma"), vec![Token::Pragma]);
        assert_eq!(tokenize("then"), vec![Token::Ident("then")]);
        assert_eq!(tokenize("true"), vec![Token::True]);
        assert_eq!(tokenize("false"), vec![Token::False]);
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
        assert_eq!(tokenize("^="), vec![Token::CaretEq]);
        assert_eq!(tokenize("&="), vec![Token::AmpEq]);
        assert_eq!(tokenize("|="), vec![Token::PipeEq]);
        assert_eq!(tokenize("%="), vec![Token::PercentEq]);
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
        assert_eq!(tokenize("&"), vec![Token::Amp]);
        assert_eq!(tokenize("^"), vec![Token::Caret]);
        assert_eq!(tokenize("@"), vec![Token::At]);
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
    fn test_hyphenated_identifiers() {
        // Hyphenated identifiers (used for pragma names).
        assert_eq!(
            tokenize("no-bounded-variable-condition"),
            vec![Token::Ident("no-bounded-variable-condition")]
        );
        assert_eq!(
            tokenize("no-patterson-condition"),
            vec![Token::Ident("no-patterson-condition")]
        );
        assert_eq!(tokenize("foo-bar"), vec![Token::Ident("foo-bar")]);
        // Regular identifiers with underscores.
        assert_eq!(tokenize("foo_bar"), vec![Token::Ident("foo_bar")]);
        // Mixed underscores and hyphens.
        assert_eq!(tokenize("foo_bar-baz"), vec![Token::Ident("foo_bar-baz")]);
        assert_eq!(tokenize("comptime"), vec![Token::Ident("comptime")]);
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

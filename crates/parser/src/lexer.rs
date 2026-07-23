//! Lexical tokens for the Solcore parser.
//!
//! Logos produces token spans in absolute byte offsets over the input string.
//! Comments and whitespace are skipped; invalid characters are reported by the
//! parser's tokenization wrapper so the rest of the grammar can recover.

use std::ops::Range;

use logos::Logos;

/// Additional state collected while lexing.
///
/// Comments remain skipped tokens as far as the parser grammar is concerned,
/// but their source ranges are retained here for declaration trivia lowering.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct LexerExtras {
    pub(crate) comments: Vec<LexedComment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexedCommentKind {
    Line,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LexedComment {
    pub(crate) kind: LexedCommentKind,
    pub(crate) range: Range<usize>,
}

/// Lexer error kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LexError {
    /// Generic invalid token.
    #[default]
    Invalid,
    /// A block comment reached end of file before its matching terminator.
    UnterminatedBlockComment,
    /// A string literal used a backslash escape not supported by the language.
    InvalidStringEscape,
}

/// Token recognized by the Solcore lexer.
///
/// Literal and identifier variants borrow slices from the input source. Token
/// ordering matters for overlapping operators: multi-character operators are
/// defined before their single-character prefixes.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\n\r\f]+")]
#[logos(error = LexError)]
#[logos(extras = LexerExtras)]
pub enum Token<'a> {
    /// `contract`.
    #[token("contract")]
    Contract,
    /// `interface`.
    #[token("interface")]
    Interface,
    /// `library`.
    #[token("library")]
    Library,
    /// `import`.
    #[token("import")]
    Import,
    /// `from`.
    #[token("from")]
    From,
    /// `export`.
    #[token("export")]
    Export,
    /// `as`.
    #[token("as")]
    As,
    /// `let`.
    #[token("let")]
    Let,
    /// `comptime`.
    #[token("comptime")]
    Comptime,
    /// `enum`.
    #[token("enum")]
    Enum,
    /// `struct`.
    #[token("struct")]
    Struct,
    /// `trait`.
    #[token("trait")]
    Trait,
    /// `impl`.
    #[token("impl")]
    Impl,
    /// `alias`.
    #[token("alias")]
    Alias,
    /// `is`.
    #[token("is")]
    Is,
    /// `where`.
    #[token("where")]
    Where,
    /// `returns`.
    #[token("returns")]
    Returns,
    /// `if`.
    #[token("if")]
    If,
    /// `else`.
    #[token("else")]
    Else,
    /// `for`.
    #[token("for")]
    For,
    /// `while`.
    #[token("while")]
    While,
    /// `unchecked`.
    #[token("unchecked")]
    Unchecked,
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
    /// `external`.
    #[token("external")]
    External,
    /// `internal`.
    #[token("internal")]
    Internal,
    /// `private`.
    #[token("private")]
    Private,
    /// `pure`.
    #[token("pure")]
    Pure,
    /// `view`.
    #[token("view")]
    View,
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
    /// `revert`.
    #[token("revert")]
    Revert,
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
    /// `~`.
    #[token("~")]
    Tilde,
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
    #[regex(r#""([^"\\]|\\.)*""#, string_literal)]
    String(&'a str),

    /// Identifier or pragma-name text.
    ///
    /// The lexer accepts hyphens so pragma names such as
    /// `no-bounded-variable-condition` tokenize as one item. The parser rejects
    /// hyphenated text in normal identifier positions.
    #[regex(r"\p{L}[\p{L}\p{N}_]*(-\p{L}[\p{L}\p{N}_]*)*", |lex| lex.slice())]
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
    let range = lex.span();
    lex.extras.comments.push(LexedComment {
        kind: LexedCommentKind::Line,
        range,
    });
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
                    let range = lex.span();
                    lex.extras.comments.push(LexedComment {
                        kind: LexedCommentKind::Block,
                        range,
                    });
                    return Ok(logos::Skip);
                }
            }
            _ => i += 1,
        }
    }

    lex.bump(remainder.len());
    Err(LexError::UnterminatedBlockComment)
}

fn string_literal<'a>(lex: &mut logos::Lexer<'a, Token<'a>>) -> Result<&'a str, LexError> {
    let slice = lex.slice();
    let mut chars = slice.chars();
    chars.next();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            break;
        }
        if ch == '\\' {
            match chars.next() {
                Some('n' | 't' | '"' | '\\') => {}
                _ => return Err(LexError::InvalidStringEscape),
            }
        }
    }
    Ok(slice)
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
        assert_eq!(tokenize("interface"), vec![Token::Interface]);
        assert_eq!(tokenize("library"), vec![Token::Library]);
        assert_eq!(tokenize("import"), vec![Token::Import]);
        assert_eq!(tokenize("from"), vec![Token::From]);
        assert_eq!(tokenize("export"), vec![Token::Export]);
        assert_eq!(tokenize("as"), vec![Token::As]);
        assert_eq!(tokenize("let"), vec![Token::Let]);
        assert_eq!(tokenize("comptime"), vec![Token::Comptime]);
        assert_eq!(tokenize("data"), vec![Token::Ident("data")]);
        assert_eq!(tokenize("enum"), vec![Token::Enum]);
        assert_eq!(tokenize("struct"), vec![Token::Struct]);
        assert_eq!(tokenize("trait"), vec![Token::Trait]);
        assert_eq!(tokenize("impl"), vec![Token::Impl]);
        assert_eq!(tokenize("class"), vec![Token::Ident("class")]);
        assert_eq!(tokenize("forall"), vec![Token::Ident("forall")]);
        assert_eq!(tokenize("instance"), vec![Token::Ident("instance")]);
        assert_eq!(tokenize("alias"), vec![Token::Alias]);
        assert_eq!(tokenize("is"), vec![Token::Is]);
        assert_eq!(tokenize("where"), vec![Token::Where]);
        assert_eq!(tokenize("returns"), vec![Token::Returns]);
        assert_eq!(tokenize("if"), vec![Token::If]);
        assert_eq!(tokenize("else"), vec![Token::Else]);
        assert_eq!(tokenize("for"), vec![Token::For]);
        assert_eq!(tokenize("while"), vec![Token::While]);
        assert_eq!(tokenize("unchecked"), vec![Token::Unchecked]);
        assert_eq!(tokenize("switch"), vec![Token::Switch]);
        assert_eq!(tokenize("type"), vec![Token::Type]);
        assert_eq!(tokenize("case"), vec![Token::Case]);
        assert_eq!(tokenize("default"), vec![Token::Default]);
        assert_eq!(tokenize("match"), vec![Token::Match]);
        assert_eq!(tokenize("public"), vec![Token::Public]);
        assert_eq!(tokenize("external"), vec![Token::External]);
        assert_eq!(tokenize("internal"), vec![Token::Internal]);
        assert_eq!(tokenize("private"), vec![Token::Private]);
        assert_eq!(tokenize("pure"), vec![Token::Pure]);
        assert_eq!(tokenize("view"), vec![Token::View]);
        assert_eq!(tokenize("payable"), vec![Token::Payable]);
        assert_eq!(tokenize("function"), vec![Token::Function]);
        assert_eq!(tokenize("constructor"), vec![Token::Constructor]);
        assert_eq!(tokenize("fallback"), vec![Token::Fallback]);
        assert_eq!(tokenize("return"), vec![Token::Return]);
        assert_eq!(tokenize("revert"), vec![Token::Revert]);
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
        assert_eq!(tokenize("~"), vec![Token::Tilde]);
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
        assert_eq!(tokenize("fλ"), vec![Token::Ident("fλ")]);
        assert_eq!(tokenize("λ2"), vec![Token::Ident("λ2")]);
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
        assert_eq!(tokenize("comptime"), vec![Token::Comptime]);
    }

    #[test]
    fn test_invalid_string_escape() {
        let mut lexer = Token::lexer(r#""bad\q""#);
        assert_eq!(lexer.next(), Some(Err(LexError::InvalidStringEscape)));
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
            tokenize("function foo(a: A, b: B) returns (C)"),
            vec![
                Token::Function,
                Token::Ident("foo"),
                Token::LParen,
                Token::Ident("a"),
                Token::Colon,
                Token::Ident("A"),
                Token::Comma,
                Token::Ident("b"),
                Token::Colon,
                Token::Ident("B"),
                Token::RParen,
                Token::Returns,
                Token::LParen,
                Token::Ident("C"),
                Token::RParen,
            ]
        );
    }

    #[test]
    fn test_contract_snippet() {
        let input = r#"
            contract Foo {
                function bar() returns (u256) {
                    let x = 0x1234;
                    return x;
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
                Token::Returns,
                Token::LParen,
                Token::Ident("u256"),
                Token::RParen,
                Token::LBrace,
                Token::Let,
                Token::Ident("x"),
                Token::Eq,
                Token::HexLit("0x1234"),
                Token::Semi,
                Token::Return,
                Token::Ident("x"),
                Token::Semi,
                Token::RBrace,
                Token::RBrace,
            ]
        );
    }
}

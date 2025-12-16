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

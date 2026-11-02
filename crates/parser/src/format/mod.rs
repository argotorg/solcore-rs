//! Token-preserving source formatting shared by the CLI and language server.
//!
//! Syntax supplies layout hints, but output always uses the original token and
//! comment slices. Printing the lowered tree would lose parentheses, spelling,
//! comments, and syntax such as compound assignments. Only gaps are rewritten.

mod syntax;

use std::{fmt, ops::Range};

use logos::Logos;

use crate::lexer::{LexedCommentKind, Token};

/// Layout preferences. Existing line endings and explicit continuation lines
/// are retained. This formatter does not yet wrap lines to a maximum width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    /// Spaces per indentation level, clamped to 1 through 16.
    pub indent_width: usize,
    /// Use spaces when true, or one tab per level when false.
    pub insert_spaces: bool,
    /// Add a final newline if absent.
    pub insert_final_newline: bool,
    /// Collapse trailing blank lines to one newline.
    pub trim_final_newlines: bool,
    /// Remove trailing whitespace outside literal and comment payloads.
    pub trim_trailing_whitespace: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            indent_width: 4,
            insert_spaces: true,
            insert_final_newline: true,
            trim_final_newlines: true,
            trim_trailing_whitespace: true,
        }
    }
}

/// A syntax or preservation error. No formatted source is returned on error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError {
    /// Absolute byte ranges and diagnostic messages in the input source.
    pub diagnostics: Vec<(Range<usize>, String)>,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (range, message)) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "{}..{}: {message}", range.start, range.end)?;
        }
        Ok(())
    }
}

impl std::error::Error for FormatError {}

#[derive(Debug, Clone, PartialEq)]
enum Kind<'a> {
    Token(Token<'a>),
    LineComment,
    BlockComment,
}

#[derive(Debug)]
struct Piece<'a> {
    kind: Kind<'a>,
    range: Range<usize>,
    text: &'a str,
}

impl Piece<'_> {
    fn token(&self) -> Option<&Token<'_>> {
        match &self.kind {
            Kind::Token(token) => Some(token),
            _ => None,
        }
    }

    fn is_comment(&self) -> bool {
        !matches!(self.kind, Kind::Token(_))
    }
}

fn pieces(source: &str) -> Result<Vec<Piece<'_>>, FormatError> {
    let mut lexer = Token::lexer(source);
    let mut pieces = Vec::new();
    while let Some(token) = lexer.next() {
        let range = lexer.span();
        let token = token.map_err(|error| FormatError {
            diagnostics: vec![(range.clone(), format!("invalid source token: {error:?}"))],
        })?;
        pieces.push(Piece {
            text: &source[range.clone()],
            range,
            kind: Kind::Token(token),
        });
    }
    for comment in lexer.extras.comments {
        let mut range = comment.range;
        // The lexer consumes the CR in a CRLF line comment. Treat it as part
        // of the line ending so a newly inserted newline cannot duplicate it.
        if comment.kind == LexedCommentKind::Line && source[range.clone()].ends_with('\r') {
            range.end -= 1;
        }
        pieces.push(Piece {
            text: &source[range.clone()],
            range,
            kind: match comment.kind {
                LexedCommentKind::Line => Kind::LineComment,
                LexedCommentKind::Block => Kind::BlockComment,
            },
        });
    }
    pieces.sort_unstable_by_key(|piece| piece.range.start);
    Ok(pieces)
}

/// Format one source file without resolving imports or checking types.
///
/// Invalid syntax is rejected. Literal and comment payloads are copied
/// verbatim, including multiline content. As a final guard, the output is
/// lexed again and must contain exactly the same token and comment sequence.
pub fn format_source(source: &str, options: &FormatOptions) -> Result<String, FormatError> {
    let input = pieces(source)?;
    let layout = syntax::layout(source).map_err(|diagnostics| FormatError { diagnostics })?;
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let indent = if options.insert_spaces {
        " ".repeat(options.indent_width.clamp(1, 16))
    } else {
        "\t".to_owned()
    };
    let mut output = String::with_capacity(source.len());
    let mut delimiters = Vec::new();
    let mut previous_code: Option<usize> = None;
    let mut unary = false;

    for (index, piece) in input.iter().enumerate() {
        let current = piece.token();
        let generic = layout.generic_angles.contains(&piece.range.start);
        let closing = matches!(
            current,
            Some(Token::RParen | Token::RBracket | Token::RBrace)
        ) || (generic && current == Some(&Token::Greater));
        if closing {
            delimiters.pop();
        }

        if let Some(previous) = index.checked_sub(1).map(|i| &input[i]) {
            let gap = &source[previous.range.end..piece.range.start];
            let code = previous_code.map(|i| &input[i]);
            let previous_block_open = code.is_some_and(|p| {
                p.token() == Some(&Token::LBrace) && layout.block_braces.contains(&p.range.start)
            });
            let empty_block =
                previous.token() == Some(&Token::LBrace) && current == Some(&Token::RBrace);
            let block_close =
                current == Some(&Token::RBrace) && layout.block_braces.contains(&piece.range.start);
            let statement_start = layout.line_starts.contains(&piece.range.start);
            let after_semi = previous.token() == Some(&Token::Semi)
                && !closing
                && !matches!(
                    delimiters.last(),
                    Some(Token::LParen | Token::LBracket | Token::Less)
                );
            let trailing_comment = piece.is_comment() && !gap.contains('\n');
            let required_break = matches!(previous.kind, Kind::LineComment)
                || (!trailing_comment
                    && !empty_block
                    && (previous_block_open || block_close || statement_start || after_semi));
            let join_else =
                current == Some(&Token::Else) && previous.token() == Some(&Token::RBrace);
            let original_breaks = if join_else {
                0
            } else {
                gap.bytes().filter(|b| *b == b'\n').count()
            };
            let breaks = original_breaks.min(2).max(usize::from(required_break));
            if breaks > 0 {
                write_breaks(
                    &mut output,
                    gap,
                    breaks,
                    newline,
                    options.trim_trailing_whitespace,
                );
                for _ in 0..delimiters.len() {
                    output.push_str(&indent);
                }
            } else if needs_space(previous, piece, &layout, unary) {
                output.push(' ');
            }
        }

        output.push_str(piece.text);
        if let Some(token) = current {
            unary = matches!(token, Token::Bang | Token::Tilde | Token::At)
                || (token == &Token::Minus
                    && !previous_code.is_some_and(|i| ends_expression(&input[i], &layout)));
            previous_code = Some(index);
            if matches!(token, Token::LParen | Token::LBracket | Token::LBrace)
                || (generic && token == &Token::Less)
            {
                delimiters.push(token.clone());
            }
        }
    }

    let tail = &source[input.last().map_or(0, |piece| piece.range.end)..];
    let breaks = tail.bytes().filter(|b| *b == b'\n').count();
    let breaks = if options.trim_final_newlines {
        breaks.min(1)
    } else {
        breaks
    };
    let breaks = breaks.max(usize::from(options.insert_final_newline));
    if breaks > 0 {
        write_breaks(
            &mut output,
            tail,
            breaks,
            newline,
            options.trim_trailing_whitespace,
        );
    } else if !options.trim_trailing_whitespace {
        output.push_str(tail);
    }
    if !options.trim_final_newlines
        && !options.trim_trailing_whitespace
        && let Some((_, final_line)) = tail.rsplit_once('\n')
    {
        output.push_str(final_line);
        if options.insert_final_newline && !output.ends_with('\n') {
            output.push_str(newline);
        }
    }

    let formatted = pieces(&output)?;
    if input.len() != formatted.len()
        || input
            .iter()
            .zip(&formatted)
            .any(|(before, after)| before.kind != after.kind || before.text != after.text)
    {
        return Err(FormatError {
            diagnostics: vec![(
                0..source.len(),
                "formatting would change source tokens or comments".to_owned(),
            )],
        });
    }
    Ok(output)
}

fn write_breaks(output: &mut String, gap: &str, count: usize, newline: &str, trim: bool) {
    let mut lines = gap.split('\n');
    for _ in 0..count {
        let line = lines.next().unwrap_or("").trim_end_matches('\r');
        if !trim {
            output.push_str(line);
        }
        output.push_str(newline);
    }
}

fn ends_expression(piece: &Piece<'_>, layout: &syntax::Layout) -> bool {
    matches!(
        piece.token(),
        Some(
            Token::Ident(_)
                | Token::YulIdent(_)
                | Token::Number(_)
                | Token::HexLit(_)
                | Token::String(_)
                | Token::True
                | Token::False
                | Token::RParen
                | Token::RBracket
                | Token::RBrace
        )
    ) || (piece.token() == Some(&Token::Greater)
        && layout.generic_angles.contains(&piece.range.start))
}

fn needs_space(
    previous: &Piece<'_>,
    current: &Piece<'_>,
    layout: &syntax::Layout,
    unary: bool,
) -> bool {
    let (Some(left), Some(right)) = (previous.token(), current.token()) else {
        return true;
    };
    if layout.pragma_hyphens.contains(&previous.range.start)
        || layout.pragma_hyphens.contains(&current.range.start)
        || layout.slice_colons.contains(&previous.range.start)
    {
        return false;
    }
    if matches!(
        right,
        Token::Comma | Token::Semi | Token::RParen | Token::RBracket
    ) || (right == &Token::Colon && !layout.ternary_colons.contains(&current.range.start))
        || (right == &Token::Dot && !layout.leading_dots.contains(&current.range.start))
        || matches!(
            left,
            Token::Dot | Token::LParen | Token::LBracket | Token::Hash
        )
    {
        return false;
    }
    if (matches!(right, Token::Less | Token::Greater)
        && layout.generic_angles.contains(&current.range.start))
        || (left == &Token::Less && layout.generic_angles.contains(&previous.range.start))
    {
        return false;
    }
    // Shift operators lex as two adjacent tokens (`<<` is `<` `<`, `<<=` is
    // `<` `<=`); separating them would change the program.
    if previous.range.end == current.range.start
        && matches!(
            (left, right),
            (Token::Less, Token::Less | Token::LessEq)
                | (Token::Greater, Token::Greater | Token::GreaterEq)
        )
    {
        return false;
    }
    if left == &Token::LBrace && right == &Token::RBrace {
        return false;
    }
    if unary {
        return false;
    }
    if right == &Token::LParen {
        return !matches!(left, Token::Ident(name) if !matches!(*name, "returns" | "while"))
            && !matches!(
                left,
                Token::YulIdent(_)
                    | Token::RParen
                    | Token::RBracket
                    | Token::Constructor
                    | Token::Fallback
            )
            && !(left == &Token::Greater && layout.generic_angles.contains(&previous.range.start));
    }
    if right == &Token::LBracket && ends_expression(previous, layout) {
        return false;
    }
    true
}

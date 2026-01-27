use chumsky::{input::ValueInput, prelude::*};
use hull::{
    Db as HullDb,
    anchor::{DefId, DefKind, DefLocation, DefLocationTable, KeyCanonicalizer},
    ast::{Ident, item, ty},
    diag::Offset,
    input::SourceFile,
    span::{AnchorId, Span, SpannedElem},
};
use logos::Logos;

pub mod lexer;
use crate::lexer::Token;

#[salsa::db]
pub trait Db: salsa::Database + HullDb {}

#[salsa::tracked(debug)]
pub struct ParseHullOutput<'db> {
    #[tracked]
    #[returns(copy)]
    pub module: item::Module<'db>,

    #[tracked]
    pub def_locations: DefLocationTable<'db>,
}

type LexSpan = chumsky::span::SimpleSpan;
type SpannedStr<'src> = (&'src str, LexSpan);
type ParserErr<'src> = extra::Err<Rich<'src, Token<'src>>>;

#[derive(Debug, Clone)]
enum ParsedTopItem<'src> {
    Import {
        span: LexSpan,
        path: Vec<SpannedStr<'src>>,
    },
    Pragma {
        span: LexSpan,
        name: SpannedStr<'src>,
        items: Vec<SpannedStr<'src>>,
    },
    TypeAlias {
        span: LexSpan,
        name: SpannedStr<'src>,
        ty: ParsedTy<'src>,
    },
    Error,
}

#[derive(Debug, Clone)]
struct ParsedTy<'src> {
    span: LexSpan,
    kind: ParsedTyKind<'src>,
}

#[derive(Debug, Clone)]
enum ParsedTyKind<'src> {
    Named {
        name: SpannedStr<'src>,
        args: Vec<ParsedTy<'src>>,
    },
}

fn ident_parser<'src, I>() -> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) => name }.validate(|name, e, emitter| {
        if name.contains('-') {
            emitter.emit(Rich::custom(
                e.span(),
                format!("identifier `{name}` cannot contain hyphens"),
            ));
        }
        (name, e.span())
    })
}

fn pragma_ident_parser<'src, I>() -> impl Parser<'src, I, SpannedStr<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    select! { Token::Ident(name) => name }.map_with(|name, e| (name, e.span()))
}

fn import_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Import)
        .ignore_then(
            ident_parser()
                .separated_by(just(Token::Dot))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(Token::Semi))
        .map_with(|path, e| ParsedTopItem::Import {
            span: e.span(),
            path,
        })
        .boxed()
}

fn pragma_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let items = ident_parser()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>();

    just(Token::Pragma)
        .ignore_then(pragma_ident_parser())
        .then(items)
        .then_ignore(just(Token::Semi))
        .map_with(|(name, items), e| ParsedTopItem::Pragma {
            span: e.span(),
            name,
            items,
        })
        .boxed()
}

fn type_parser<'src, I>() -> impl Parser<'src, I, ParsedTy<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    recursive(|ty| {
        let args = ty
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .or_not()
            .map(|args| args.unwrap_or_default())
            .boxed();

        ident_parser()
            .then(args)
            .map_with(|(name, args), e| ParsedTy {
                span: e.span(),
                kind: ParsedTyKind::Named { name, args },
            })
            .boxed()
    })
}

fn type_alias_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    just(Token::Type)
        .ignore_then(ident_parser())
        .then_ignore(just(Token::Eq))
        .then(type_parser())
        .then_ignore(just(Token::Semi))
        .map_with(|(name, ty), e| ParsedTopItem::TypeAlias {
            span: e.span(),
            name,
            ty,
        })
        .boxed()
}

fn top_item_parser<'src, I>() -> impl Parser<'src, I, ParsedTopItem<'src>, ParserErr<'src>>
where
    I: ValueInput<'src, Token = Token<'src>, Span = LexSpan>,
{
    let item_start = just(Token::Import)
        .or(just(Token::Pragma))
        .or(just(Token::Type));
    let recovery = any()
        .and_is(item_start.not())
        .repeated()
        .at_least(1)
        .to(ParsedTopItem::Error);

    choice((import_parser(), pragma_parser(), type_alias_parser()))
        .recover_with(via_parser(recovery))
}

fn tokenize<'src>(src: &'src str) -> Vec<(Token<'src>, LexSpan)> {
    Token::lexer(src)
        .spanned()
        .filter_map(|(tok, span)| tok.ok().map(|tok| (tok, LexSpan::from(span))))
        .collect()
}

fn parse_supported_items<'src>(src: &'src str) -> Vec<ParsedTopItem<'src>> {
    let tokens = tokenize(src);
    let stream = chumsky::input::Stream::from_iter(tokens)
        .map((0..src.len()).into(), |(tok, span): (_, _)| (tok, span));

    top_item_parser()
        .repeated()
        .collect::<Vec<_>>()
        .parse(stream)
        .into_result()
        .unwrap_or_default()
}

fn offset_from_usize(raw: usize) -> Offset {
    Offset::try_from_usize(raw).expect("span offset exceeds u32::MAX")
}

fn span_from_absolute<'db>(anchor: AnchorId<'db>, abs: LexSpan, base_start: usize) -> Span<'db> {
    let rel_start = abs
        .start
        .checked_sub(base_start)
        .expect("span start is before anchor base");
    let rel_end = abs
        .end
        .checked_sub(base_start)
        .expect("span end is before anchor base");
    Span::new(
        anchor,
        offset_from_usize(rel_start),
        offset_from_usize(rel_end),
    )
}

fn lower_spanned_ident<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    (name, span): SpannedStr<'_>,
) -> SpannedElem<'db, Ident<'db>> {
    SpannedElem::new(
        Ident::new(db, name.to_owned()),
        span_from_absolute(anchor, span, base_start),
    )
}

fn lower_import<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    path: Vec<SpannedStr<'_>>,
) -> item::Import<'db> {
    let import_def = keys.alloc_def(db, file, DefKind::Import, None);
    def_locations.push((
        import_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, import_def);
    let path = path
        .into_iter()
        .map(|segment| lower_spanned_ident(db, anchor, span.start, segment))
        .collect();
    let span = span_from_absolute(anchor, span, span.start);
    item::Import::new(db, import_def, span, path)
}

fn lower_pragma<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    name: SpannedStr<'_>,
    items: Vec<SpannedStr<'_>>,
) -> item::Pragma<'db> {
    let pragma_def = keys.alloc_def(db, file, DefKind::Pragma, Some(name.0));
    def_locations.push((
        pragma_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, pragma_def);
    let name = lower_spanned_ident(db, anchor, span.start, name);
    let items = items
        .into_iter()
        .map(|segment| lower_spanned_ident(db, anchor, span.start, segment))
        .collect();
    let span = span_from_absolute(anchor, span, span.start);
    item::Pragma::new(db, pragma_def, span, name, items)
}

fn lower_type_ref<'db>(
    db: &'db dyn Db,
    anchor: AnchorId<'db>,
    base_start: usize,
    parsed_ty: ParsedTy<'_>,
) -> ty::TypeRef<'db> {
    let kind = match parsed_ty.kind {
        ParsedTyKind::Named { name, args } => {
            let name = lower_spanned_ident(db, anchor, base_start, name);
            let args = args
                .into_iter()
                .map(|arg| lower_type_ref(db, anchor, base_start, arg))
                .collect::<Vec<_>>();
            let args_span = span_from_absolute(anchor, parsed_ty.span, base_start);
            ty::TypeRefKind::Named {
                name,
                args: SpannedElem::new(args, args_span),
            }
        }
    };
    ty::TypeRef::new(db, kind)
}

fn lower_type_alias<'db>(
    db: &'db dyn Db,
    file: SourceFile,
    keys: &mut KeyCanonicalizer,
    def_locations: &mut Vec<(DefId<'db>, DefLocation)>,
    span: LexSpan,
    name: SpannedStr<'_>,
    parsed_ty: ParsedTy<'_>,
) -> item::TypeAlias<'db> {
    let alias_def = keys.alloc_def(db, file, DefKind::TypeAlias, Some(name.0));
    def_locations.push((
        alias_def,
        DefLocation {
            file,
            base_offset: offset_from_usize(span.start),
        },
    ));

    let anchor = AnchorId::def(db, alias_def);
    let name = lower_spanned_ident(db, anchor, span.start, name);
    let ty = lower_type_ref(db, anchor, span.start, parsed_ty);
    let span = span_from_absolute(anchor, span, span.start);
    item::TypeAlias::new(db, alias_def, span, name, ty)
}

/// Parses one source file into Hull IR in a single pass.
#[salsa::tracked]
pub fn parse_file_to_hull<'db>(db: &'db dyn Db, file: SourceFile) -> ParseHullOutput<'db> {
    let mut keys = KeyCanonicalizer::new();
    let module_def = keys.alloc_def(db, file, DefKind::Module, None);

    let source = file.content(db).as_deref().unwrap_or("");
    let end = offset_from_usize(source.len());
    let module_span = Span::new(AnchorId::root(db, file), Offset::new(0), end);

    let mut items = Vec::new();
    let mut def_locations = vec![(
        module_def,
        DefLocation {
            file,
            base_offset: Offset::new(0),
        },
    )];

    for parsed in parse_supported_items(source) {
        match parsed {
            ParsedTopItem::Import { span, path } => {
                let import = lower_import(db, file, &mut keys, &mut def_locations, span, path);
                items.push(item::Item::Import(import));
            }
            ParsedTopItem::Pragma {
                span,
                name,
                items: pragma_items,
            } => {
                let pragma = lower_pragma(
                    db,
                    file,
                    &mut keys,
                    &mut def_locations,
                    span,
                    name,
                    pragma_items,
                );
                items.push(item::Item::Pragma(pragma));
            }
            ParsedTopItem::TypeAlias { span, name, ty } => {
                let alias =
                    lower_type_alias(db, file, &mut keys, &mut def_locations, span, name, ty);
                items.push(item::Item::TypeAlias(alias));
            }
            ParsedTopItem::Error => {}
        }
    }

    let module = item::Module::new(db, module_def, module_span, items);
    let def_locations = DefLocationTable::from_def_locations(def_locations);

    ParseHullOutput::new(db, module, def_locations)
}

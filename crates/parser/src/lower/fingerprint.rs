use crate::types::*;

use super::span::path_text;

pub(super) fn import_fingerprint(
    external: Option<LexSpan>,
    path: &[SpannedStr<'_>],
    alias: Option<&SpannedStr<'_>>,
    selector: Option<&ParsedImportSelector<'_>>,
    hiding: &[ParsedImportName],
) -> String {
    // Import identity is based on normalized import semantics, not the byte
    // location of the declaration. Selector and hiding lists are sorted so
    // reordering names does not churn the DefId.
    let mut fingerprint = if external.is_some() {
        "@".to_owned()
    } else {
        String::new()
    };
    fingerprint.push_str(
        &path
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join("."),
    );

    if let Some((alias, _)) = alias {
        fingerprint.push_str(" as ");
        fingerprint.push_str(alias);
    }

    if let Some(selector) = selector {
        match selector {
            ParsedImportSelector::Wildcard => fingerprint.push_str(".{*}"),
            ParsedImportSelector::Names(names) => {
                fingerprint.push_str(".{");
                fingerprint.push_str(&sorted_fingerprints(names, selected_fingerprint));
                fingerprint.push('}');
            }
        }
    }

    if !hiding.is_empty() {
        fingerprint.push_str(" hiding {");
        fingerprint.push_str(&sorted_fingerprints(hiding, import_name_fingerprint));
        fingerprint.push('}');
    }

    fingerprint
}

fn selected_fingerprint(name: &ParsedSelectedName<'_>) -> String {
    let mut fingerprint = import_name_fingerprint(&name.name);
    if let Some(constructors) = &name.constructors {
        fingerprint.push_str(&constructor_selector_fingerprint(constructors));
    }
    if let Some((alias, _)) = &name.alias {
        fingerprint.push_str(" as ");
        fingerprint.push_str(alias);
    }
    fingerprint
}

fn constructor_selector_fingerprint(selector: &ParsedConstructorSelector<'_>) -> String {
    match selector {
        ParsedConstructorSelector::All => "(*)".to_owned(),
        ParsedConstructorSelector::Named(names) => {
            let mut names = names.iter().map(|(name, _)| *name).collect::<Vec<_>>();
            names.sort_unstable();
            format!("({})", names.join(","))
        }
    }
}

fn import_name_fingerprint(name: &ParsedImportName) -> String {
    let kind = if name.is_operator { "op" } else { "name" };
    format!("{kind}:{}", name.name)
}

pub(super) fn export_fingerprint(kind: &ParsedExportKind<'_>) -> String {
    match kind {
        ParsedExportKind::List(names) => {
            format!(
                "list{{{}}}",
                sorted_fingerprints(names, export_name_fingerprint)
            )
        }
        ParsedExportKind::Module(path) => format!("module {}", path_fingerprint(path)),
        ParsedExportKind::ModuleAs(path, alias) => {
            format!("module {} as {}", path_fingerprint(path), alias.0)
        }
        ParsedExportKind::ItemsFrom(path, names) => {
            format!(
                "items {}.{{{}}}",
                path_fingerprint(path),
                sorted_fingerprints(names, export_name_fingerprint)
            )
        }
    }
}

fn export_name_fingerprint(name: &ParsedExportName<'_>) -> String {
    let mut fingerprint = import_name_fingerprint(&name.name);
    if let Some(constructors) = &name.constructors {
        fingerprint.push_str(&constructor_selector_fingerprint(constructors));
    }
    fingerprint
}

fn path_fingerprint(path: &[SpannedStr<'_>]) -> String {
    path.iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(".")
}

fn sorted_fingerprints<T>(items: &[T], fingerprint: fn(&T) -> String) -> String {
    let mut fingerprints = items.iter().map(fingerprint).collect::<Vec<_>>();
    fingerprints.sort_unstable();
    fingerprints.join(",")
}

fn source_snippet_fingerprint(source: &str, span: LexSpan) -> String {
    source.get(span.start..span.end).unwrap_or("").to_owned()
}

fn optional_ty_snippet_fingerprint(source: &str, ty: Option<&ParsedTy<'_>>) -> String {
    ty.map(|ty| source_snippet_fingerprint(source, ty.span))
        .unwrap_or_else(|| "<none>".to_owned())
}

pub(super) fn lambda_fingerprint(
    source: &str,
    params_span: LexSpan,
    ret: Option<&ParsedTy<'_>>,
) -> String {
    structural_fingerprint(
        "lambda",
        &[
            source_snippet_fingerprint(source, params_span),
            optional_ty_snippet_fingerprint(source, ret),
        ],
    )
}

pub(super) fn instance_head_fingerprint(
    type_vars: &[SpannedStr<'_>],
    head: &ParsedPred<'_>,
) -> Option<String> {
    let type_vars = type_vars
        .iter()
        .enumerate()
        .map(|(index, (name, _))| (*name, index))
        .collect::<Vec<_>>();

    let mut components = Vec::with_capacity(1 + head.args.len());
    components.push(canonical_ty_fingerprint(&head.ty, &type_vars)?);
    for arg in &head.args {
        components.push(canonical_ty_fingerprint(arg, &type_vars)?);
    }
    Some(structural_fingerprint("pred", &components))
}

fn structural_fingerprint(label: &str, components: &[String]) -> String {
    // Length prefixes make the encoding unambiguous even when component strings
    // contain punctuation used by the fingerprint syntax.
    let mut fingerprint = format!("{label}[{}]", components.len());
    for component in components {
        fingerprint.push('|');
        fingerprint.push_str(&component.len().to_string());
        fingerprint.push(':');
        fingerprint.push_str(component);
    }
    fingerprint
}

fn canonical_ty_fingerprint(ty: &ParsedTy<'_>, type_vars: &[(&str, usize)]) -> Option<String> {
    match &ty.kind {
        ParsedTyKind::Named {
            qualifiers,
            name,
            args,
            args_span: _,
        } => {
            let name = if args.is_empty() && qualifiers.is_empty() {
                // Instance identity is alpha-equivalent over its declared type
                // variables, so binders are encoded by position rather than by
                // surface spelling.
                type_vars
                    .iter()
                    .find_map(|(var, index)| (*var == name.0).then_some(format!("${index}")))
                    .unwrap_or_else(|| name.0.to_owned())
            } else if qualifiers.is_empty() {
                name.0.to_owned()
            } else {
                format!("{}.{}", path_text(qualifiers), name.0)
            };

            if args.is_empty() {
                Some(name)
            } else {
                let args = args
                    .iter()
                    .map(|arg| canonical_ty_fingerprint(arg, type_vars))
                    .collect::<Option<Vec<_>>>()?;
                Some(format!("{name}({})", args.join(",")))
            }
        }
        ParsedTyKind::Proxy { inner, .. } => {
            canonical_ty_fingerprint(inner, type_vars).map(|inner| format!("Proxy({inner})"))
        }
        ParsedTyKind::Fn {
            params,
            params_span: _,
            ret,
        } => {
            let params = params
                .iter()
                .map(|param| canonical_ty_fingerprint(param, type_vars))
                .collect::<Option<Vec<_>>>()?;
            let ret = canonical_ty_fingerprint(ret, type_vars)?;
            Some(format!("fn({})->{ret}", params.join(",")))
        }
        ParsedTyKind::Comptime { inner, .. } => {
            canonical_ty_fingerprint(inner, type_vars).map(|inner| format!("comptime({inner})"))
        }
        ParsedTyKind::Tuple { elems } => {
            let elems = elems
                .iter()
                .map(|elem| canonical_ty_fingerprint(elem, type_vars))
                .collect::<Option<Vec<_>>>()?;
            Some(format!("({})", elems.join(",")))
        }
        ParsedTyKind::Error => None,
    }
}

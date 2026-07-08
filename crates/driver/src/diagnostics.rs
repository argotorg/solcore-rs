use std::{env, io::IsTerminal};

use annotate_snippets::{Renderer, renderer::DecorStyle};
use hir::diag::{Diagnostic, DiagnosticLevel};

use crate::args::{
    Args, ColorChoice, DiagnosticFormat, UnicodeChoice, WarningPolicy, default_diagnostic_width,
};

fn diagnostic_renderer(args: &Args) -> Renderer {
    let renderer = match args.color {
        ColorChoice::Always => Renderer::styled(),
        ColorChoice::Never => Renderer::plain(),
        ColorChoice::Auto => {
            let no_color = env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
            if !no_color && std::io::stderr().is_terminal() {
                Renderer::styled()
            } else {
                Renderer::plain()
            }
        }
    };
    renderer
        .term_width(
            args.diagnostic_width
                .unwrap_or_else(default_diagnostic_width),
        )
        .decor_style(match args.unicode {
            UnicodeChoice::Always => DecorStyle::Unicode,
            UnicodeChoice::Never => DecorStyle::Ascii,
            UnicodeChoice::Auto if std::io::stderr().is_terminal() => DecorStyle::Unicode,
            UnicodeChoice::Auto => DecorStyle::Ascii,
        })
}

pub(crate) fn render_diagnostics(
    db: &dyn hir::Db,
    diagnostics: &[Diagnostic],
    args: &Args,
) -> String {
    match args.diagnostic_format {
        DiagnosticFormat::Human => {
            let renderer = diagnostic_renderer(args);
            render_diagnostic_blocks(
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.render_with(db, &renderer)),
            )
        }
        DiagnosticFormat::Short => diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render_short(db))
            .collect(),
    }
}

fn render_diagnostic_blocks(rendered_blocks: impl IntoIterator<Item = String>) -> String {
    let mut output = String::new();
    for rendered in rendered_blocks {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&normalize_rendered_diagnostic(rendered));
    }
    output
}

fn normalize_rendered_diagnostic(mut rendered: String) -> String {
    while rendered.ends_with('\n') {
        rendered.pop();
    }
    rendered.push('\n');
    rendered
}

pub(crate) fn apply_warning_policy(diagnostics: &mut Vec<Diagnostic>, policy: WarningPolicy) {
    match policy {
        WarningPolicy::Default | WarningPolicy::Always => {}
        WarningPolicy::Never => {
            diagnostics.retain(|diagnostic| diagnostic.level != DiagnosticLevel::Warning);
        }
        WarningPolicy::Deny => {
            for diagnostic in diagnostics
                .iter_mut()
                .filter(|diagnostic| diagnostic.level == DiagnosticLevel::Warning)
            {
                diagnostic.level = DiagnosticLevel::Error;
                diagnostic.notes.push(
                    "pass --warnings=default, --warnings=always, or --warnings=never to allow this warning"
                        .to_owned(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_diagnostic_blocks_have_rustc_style_spacing() {
        assert_eq!(
            render_diagnostic_blocks(["error: one".to_owned()]),
            "error: one\n"
        );
        assert_eq!(
            render_diagnostic_blocks(["error: one\n\n".to_owned(), "error: two".to_owned()]),
            "error: one\n\nerror: two\n"
        );
    }
}

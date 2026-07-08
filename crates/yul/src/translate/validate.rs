use crate::{
    ast::{Code, Expr, Inner, Literal, Object, Program, Stmt},
    pretty::pretty_object,
};

use super::{
    TranslationError,
    names::{
        canonical_hex_lit, canonical_numeric_lit, is_forbidden_yul_identifier,
        is_valid_yul_identifier,
    },
};

pub(super) fn render_strict_assembly_program(
    program: &Program,
    object_name: Option<&str>,
) -> Result<String, TranslationError> {
    let object = select_strict_object(program, object_name)?;
    validate_object(object)?;
    Ok(pretty_object(object))
}

fn select_strict_object<'a>(
    program: &'a Program,
    object_name: Option<&str>,
) -> Result<&'a Object, TranslationError> {
    if let Some(name) = object_name {
        return program
            .objects
            .iter()
            .find(|object| object.name.as_str() == name)
            .ok_or_else(|| {
                TranslationError::new(format!(
                    "Yul object `{name}` not found; available top-level objects: {}",
                    top_level_object_list(program)
                ))
            });
    }

    match program.objects.as_slice() {
        [object] => Ok(object),
        [] => Err(TranslationError::new(
            "strict-assembly output requires one top-level object; found none",
        )),
        _ => Err(TranslationError::new(format!(
            "strict-assembly output requires one top-level object; found {} ({})",
            program.objects.len(),
            top_level_object_list(program)
        ))),
    }
}

fn top_level_object_list(program: &Program) -> String {
    program
        .objects
        .iter()
        .map(|object| object.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, Copy)]
enum ControlRegion {
    Outside,
    LoopInit,
    LoopPost,
    LoopBody,
}

fn validate_object(object: &Object) -> Result<(), TranslationError> {
    validate_code(&object.code)?;
    for inner in &object.inners {
        match inner {
            Inner::Object(object) => validate_object(object)?,
            Inner::Data(_) => {}
        }
    }
    Ok(())
}

fn validate_code(code: &Code) -> Result<(), TranslationError> {
    validate_stmts(&code.stmts, ControlRegion::Outside)
}

fn validate_stmts(stmts: &[Stmt], region: ControlRegion) -> Result<(), TranslationError> {
    for stmt in stmts {
        validate_stmt(stmt, region)?;
    }
    Ok(())
}

fn validate_stmt(stmt: &Stmt, region: ControlRegion) -> Result<(), TranslationError> {
    match stmt {
        Stmt::Block(stmts) => validate_stmts(stmts, region),
        Stmt::Function {
            name,
            params,
            returns,
            body,
        } => {
            validate_decl_name(name.as_str())?;
            for name in params.iter().chain(returns) {
                validate_decl_name(name.as_str())?;
            }
            validate_stmts(body, ControlRegion::Outside)
        }
        Stmt::Let { names, init } => {
            for name in names {
                validate_decl_name(name.as_str())?;
            }
            if let Some(init) = init {
                validate_expr(init)?;
            }
            Ok(())
        }
        Stmt::Assign { names, value } => {
            for name in names {
                validate_decl_name(name.as_str())?;
            }
            validate_expr(value)
        }
        Stmt::If { cond, body } => {
            validate_expr(cond)?;
            validate_stmts(body, region)
        }
        Stmt::Switch {
            expr,
            cases,
            default,
        } => {
            validate_expr(expr)?;
            for case in cases {
                validate_lit(&case.lit)?;
                validate_stmts(&case.body, region)?;
            }
            if let Some(default) = default {
                validate_stmts(default, region)?;
            }
            Ok(())
        }
        Stmt::For {
            init,
            cond,
            post,
            body,
        } => {
            validate_stmts(init, ControlRegion::LoopInit)?;
            validate_expr(cond)?;
            validate_stmts(post, ControlRegion::LoopPost)?;
            validate_stmts(body, ControlRegion::LoopBody)
        }
        Stmt::Break => validate_break_continue("break", region),
        Stmt::Continue => validate_break_continue("continue", region),
        Stmt::Leave | Stmt::Comment(_) => Ok(()),
        Stmt::Expr(expr) => validate_expr(expr),
    }
}

fn validate_break_continue(keyword: &str, region: ControlRegion) -> Result<(), TranslationError> {
    match region {
        ControlRegion::LoopBody => Ok(()),
        ControlRegion::LoopInit => Err(TranslationError::new(format!(
            "`{keyword}` in for-loop init block is not allowed"
        ))),
        ControlRegion::LoopPost => Err(TranslationError::new(format!(
            "`{keyword}` in for-loop post block is not allowed"
        ))),
        ControlRegion::Outside => Err(TranslationError::new(format!(
            "`{keyword}` must be inside a for-loop body"
        ))),
    }
}

fn validate_expr(expr: &Expr) -> Result<(), TranslationError> {
    match expr {
        Expr::Call { name, args } => {
            validate_call_name(name.as_str())?;
            for arg in args {
                validate_expr(arg)?;
            }
            Ok(())
        }
        Expr::Ident(name) => validate_decl_name(name.as_str()),
        Expr::Lit(lit) => validate_lit(lit),
    }
}

fn validate_lit(lit: &Literal) -> Result<(), TranslationError> {
    match lit {
        Literal::Number(value) => canonical_numeric_lit(value).map(|_| ()),
        Literal::Hex(value) => canonical_hex_lit(value).map(|_| ()),
        Literal::String(_) | Literal::Bool(_) => Ok(()),
    }
}

fn validate_decl_name(name: &str) -> Result<(), TranslationError> {
    if !is_valid_yul_identifier(name) {
        return Err(TranslationError::new(format!(
            "invalid Yul identifier `{name}`"
        )));
    }
    if is_forbidden_yul_identifier(name) {
        return Err(TranslationError::new(format!(
            "Yul identifier `{name}` is reserved or builtin"
        )));
    }
    Ok(())
}

fn validate_call_name(name: &str) -> Result<(), TranslationError> {
    if is_valid_yul_identifier(name) {
        Ok(())
    } else {
        Err(TranslationError::new(format!(
            "invalid Yul function name `{name}`"
        )))
    }
}

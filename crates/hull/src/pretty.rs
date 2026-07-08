use std::fmt::Write as _;

use hir::{
    Db as HirDb,
    ast::function::{YulCase, YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
};

use crate::ir::{
    Alt, Arg, CodeBlock, Con, Expr, ExprKind, Function, Object, Pat, PatKind, Program, Stmt,
    StmtKind, Ty, TyKind,
};

pub trait PrettyHull<'db> {
    fn to_hull_string(&self, db: &'db dyn HirDb) -> String;
}

pub fn pretty_program<'db>(db: &'db dyn HirDb, program: &Program<'db>) -> String {
    program.to_hull_string(db)
}

impl<'db> PrettyHull<'db> for Program<'db> {
    fn to_hull_string(&self, db: &'db dyn HirDb) -> String {
        let mut out = String::new();
        for (index, function) in self.functions.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            write_function(db, &mut out, function, 0);
        }
        if !self.functions.is_empty() && !self.objects.is_empty() {
            out.push('\n');
        }
        for (index, object) in self.objects.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            write_object(db, &mut out, object, 0);
        }
        out
    }
}

impl<'db> PrettyHull<'db> for Ty<'db> {
    fn to_hull_string(&self, _db: &'db dyn HirDb) -> String {
        write_ty(self)
    }
}

impl<'db> PrettyHull<'db> for Expr<'db> {
    fn to_hull_string(&self, _db: &'db dyn HirDb) -> String {
        write_expr(self)
    }
}

fn write_object<'db>(db: &'db dyn HirDb, out: &mut String, object: &Object<'db>, indent: usize) {
    line(
        out,
        indent,
        &format!("object \"{}\" {{", escape_string(object.name.as_str())),
    );
    line(out, indent + 1, "code {");
    write_code_block(db, out, &object.code, indent + 2);
    line(out, indent + 1, "}");
    for inner in &object.inners {
        write_object(db, out, inner, indent + 1);
    }
    line(out, indent, "}");
}

fn write_code_block<'db>(
    db: &'db dyn HirDb,
    out: &mut String,
    code: &CodeBlock<'db>,
    indent: usize,
) {
    for function in &code.functions {
        write_function(db, out, function, indent);
    }
    for stmt in &code.stmts {
        write_stmt(db, out, stmt, indent);
    }
}

fn write_function<'db>(
    db: &'db dyn HirDb,
    out: &mut String,
    function: &Function<'db>,
    indent: usize,
) {
    let args = function
        .args
        .iter()
        .map(write_arg)
        .collect::<Vec<_>>()
        .join(", ");
    line(
        out,
        indent,
        &format!(
            "function {} ({}) -> {} {{",
            function.name,
            args,
            write_ty(&function.ret)
        ),
    );
    for stmt in &function.body {
        write_stmt(db, out, stmt, indent + 1);
    }
    line(out, indent, "}");
}

fn write_arg<'db>(arg: &Arg<'db>) -> String {
    format!("{} : {}", arg.name, write_ty(&arg.ty))
}

fn write_stmt<'db>(db: &'db dyn HirDb, out: &mut String, stmt: &Stmt<'db>, indent: usize) {
    match &stmt.kind {
        StmtKind::Let { name, ty } => line(out, indent, &format!("let {name} : {}", write_ty(ty))),
        StmtKind::Assign { lhs, rhs } => line(
            out,
            indent,
            &format!("{} := {}", write_expr(lhs), write_expr(rhs)),
        ),
        StmtKind::Expr(expr) => line(out, indent, &write_expr(expr)),
        StmtKind::Return(expr) => line(out, indent, &format!("return {}", write_expr(expr))),
        StmtKind::Block(stmts) => {
            line(out, indent, "{");
            for stmt in stmts {
                write_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        StmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            line(
                out,
                indent,
                &format!(
                    "for ({}; {}; {}) {{",
                    write_stmt_list_inline(init),
                    write_expr(cond),
                    write_stmt_list_inline(post)
                ),
            );
            for stmt in body {
                write_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        StmtKind::Break => line(out, indent, "break"),
        StmtKind::Continue => line(out, indent, "continue"),
        StmtKind::Match {
            target,
            scrutinee,
            alts,
        } => {
            line(
                out,
                indent,
                &format!(
                    "match<{}> {} with {{",
                    write_ty(target),
                    write_expr(scrutinee)
                ),
            );
            for alt in alts {
                write_alt(db, out, alt, indent + 1);
            }
            line(out, indent, "}");
        }
        StmtKind::Assembly(stmts) => {
            line(out, indent, "assembly {");
            for stmt in stmts {
                write_yul_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        StmtKind::Revert(message) => {
            line(
                out,
                indent,
                &format!("revertLit \"{}\"", escape_string(message)),
            );
        }
        StmtKind::Comment(comment) => {
            line(
                out,
                indent,
                &format!("/* {} */", comment.replace("*/", "* /")),
            );
        }
    }
}

fn write_stmt_list_inline(stmts: &[Stmt<'_>]) -> String {
    match stmts {
        [] => "{}".to_owned(),
        [stmt] => write_stmt_inline(stmt),
        _ => {
            let body = stmts
                .iter()
                .map(write_stmt_inline)
                .collect::<Vec<_>>()
                .join(" ");
            format!("{{ {body} }}")
        }
    }
}

fn write_stmt_inline(stmt: &Stmt<'_>) -> String {
    match &stmt.kind {
        StmtKind::Let { name, ty } => format!("let {name} : {}", write_ty(ty)),
        StmtKind::Assign { lhs, rhs } => format!("{} := {}", write_expr(lhs), write_expr(rhs)),
        StmtKind::Expr(expr) => write_expr(expr),
        StmtKind::Return(expr) => format!("return {}", write_expr(expr)),
        StmtKind::Block(stmts) => {
            if stmts.is_empty() {
                "{}".to_owned()
            } else {
                let body = stmts
                    .iter()
                    .map(write_stmt_inline)
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{{ {body} }}")
            }
        }
        StmtKind::Break => "break".to_owned(),
        StmtKind::Continue => "continue".to_owned(),
        StmtKind::Revert(message) => format!("revertLit \"{}\"", escape_string(message)),
        StmtKind::Comment(comment) => format!("/* {} */", comment.replace("*/", "* /")),
        StmtKind::For { .. } | StmtKind::Match { .. } | StmtKind::Assembly(_) => "{}".to_owned(),
    }
}

fn write_alt<'db>(db: &'db dyn HirDb, out: &mut String, alt: &Alt<'db>, indent: usize) {
    line(
        out,
        indent,
        &format!("{} {} => {{", write_pat(&alt.pat), alt.binder),
    );
    for stmt in &alt.body {
        write_stmt(db, out, stmt, indent + 1);
    }
    line(out, indent, "}");
}

fn write_ty<'db>(ty: &Ty<'db>) -> String {
    match &ty.kind {
        TyKind::Word => "word".to_owned(),
        TyKind::Bool => "bool".to_owned(),
        TyKind::Unit => "unit".to_owned(),
        TyKind::Product(lhs, rhs) => format!("({} * {})", write_ty(lhs), write_ty(rhs)),
        TyKind::Sum(lhs, rhs) => format!("({} + {})", write_ty(lhs), write_ty(rhs)),
        TyKind::Named { name, inner } => format!("{name}{{{}}}", write_ty(inner)),
        TyKind::NamedRef { name } => name.as_str().to_owned(),
        TyKind::Function { params, ret } => {
            let params = params.iter().map(write_ty).collect::<Vec<_>>().join(", ");
            format!("({params} -> {})", write_ty(ret))
        }
    }
}

fn write_expr<'db>(expr: &Expr<'db>) -> String {
    match &expr.kind {
        ExprKind::Word(value) => value.clone(),
        ExprKind::Bool(value) => value.to_string(),
        ExprKind::Unit => "()".to_owned(),
        ExprKind::Var(name) => name.as_str().to_owned(),
        ExprKind::Pair(lhs, rhs) => format!("({}, {})", write_expr(lhs), write_expr(rhs)),
        ExprKind::Fst(expr) => format!("fst({})", write_expr(expr)),
        ExprKind::Snd(expr) => format!("snd({})", write_expr(expr)),
        ExprKind::Inl { target, value } => {
            format!("inl<{}>({})", write_ty(target), write_expr(value))
        }
        ExprKind::Inr { target, value } => {
            format!("inr<{}>({})", write_ty(target), write_expr(value))
        }
        ExprKind::InK {
            index,
            target,
            value,
        } => format!("in({index})<{}>({})", write_ty(target), write_expr(value)),
        ExprKind::Call { callee, args } => {
            let args = args.iter().map(write_expr).collect::<Vec<_>>().join(", ");
            format!("{callee}({args})")
        }
        ExprKind::If {
            target,
            cond,
            then_expr,
            else_expr,
        } => format!(
            "if<{}> {} then ({}) else ({})",
            write_ty(target),
            write_expr(cond),
            write_expr(then_expr),
            write_expr(else_expr)
        ),
    }
}

fn write_pat(pat: &Pat<'_>) -> String {
    match &pat.kind {
        PatKind::Var(name) => name.as_str().to_owned(),
        PatKind::Con(con) => match con {
            Con::Inl => "inl".to_owned(),
            Con::Inr => "inr".to_owned(),
            Con::InK(index) => format!("in({index})"),
        },
        PatKind::Wildcard => "_".to_owned(),
        PatKind::IntLit(value) => value.clone(),
    }
}

fn write_yul_stmt<'db>(db: &'db dyn HirDb, out: &mut String, stmt: &YulStmt<'db>, indent: usize) {
    match &stmt.kind {
        YulStmtKind::Block(stmts) => {
            line(out, indent, "{");
            for stmt in stmts {
                write_yul_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        YulStmtKind::Let { names, init } => {
            let names = yul_names(db, names);
            match init {
                Some(init) => line(
                    out,
                    indent,
                    &format!("let {names} := {}", yul_expr(db, init)),
                ),
                None => line(out, indent, &format!("let {names}")),
            }
        }
        YulStmtKind::Assign { names, value } => line(
            out,
            indent,
            &format!("{} := {}", yul_names(db, names), yul_expr(db, value)),
        ),
        YulStmtKind::Expr(expr) => line(out, indent, &yul_expr(db, expr)),
        YulStmtKind::If { cond, body } => {
            line(out, indent, &format!("if {} {{", yul_expr(db, cond)));
            for stmt in body {
                write_yul_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        YulStmtKind::For {
            init,
            cond,
            post,
            body,
        } => {
            line(out, indent, "for {");
            for stmt in init {
                write_yul_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, &format!("}} {} {{", yul_expr(db, cond)));
            for stmt in post {
                write_yul_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, "} {");
            for stmt in body {
                write_yul_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        YulStmtKind::Switch {
            expr,
            cases,
            default,
        } => {
            line(out, indent, &format!("switch {}", yul_expr(db, expr)));
            for case in cases {
                write_yul_case(db, out, case, indent + 1);
            }
            if let Some(default) = default {
                line(out, indent + 1, "default {");
                for stmt in default {
                    write_yul_stmt(db, out, stmt, indent + 2);
                }
                line(out, indent + 1, "}");
            }
        }
        YulStmtKind::FunctionDef {
            name,
            params,
            rets,
            body,
        } => {
            let name = (*name.atom()).text(db);
            let params = yul_names(db, params);
            let rets = yul_names(db, rets);
            let ret = if rets.is_empty() {
                String::new()
            } else {
                format!(" -> {rets}")
            };
            line(out, indent, &format!("function {name}({params}){ret} {{"));
            for stmt in body {
                write_yul_stmt(db, out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        YulStmtKind::Leave => line(out, indent, "leave"),
        YulStmtKind::Break => line(out, indent, "break"),
        YulStmtKind::Continue => line(out, indent, "continue"),
        YulStmtKind::Error => line(out, indent, "<error>"),
    }
}

fn write_yul_case<'db>(db: &'db dyn HirDb, out: &mut String, case: &YulCase<'db>, indent: usize) {
    line(out, indent, &format!("case {} {{", yul_lit(&case.lit)));
    for stmt in &case.body {
        write_yul_stmt(db, out, stmt, indent + 1);
    }
    line(out, indent, "}");
}

fn yul_names<'db>(
    db: &'db dyn HirDb,
    names: &[hir::span::SpannedElem<'db, hir::ast::Ident<'db>>],
) -> String {
    names
        .iter()
        .map(|name| (*name.atom()).text(db).to_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

fn yul_expr<'db>(db: &'db dyn HirDb, expr: &YulExpr<'db>) -> String {
    match &expr.kind {
        YulExprKind::Lit(lit) => yul_lit(lit),
        YulExprKind::Ident(name) => (*name.atom()).text(db).to_owned(),
        YulExprKind::Call { name, args } => {
            let name = (*name.atom()).text(db);
            let args = args
                .iter()
                .map(|arg| yul_expr(db, arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{name}({args})")
        }
        YulExprKind::Error => "<error>".to_owned(),
    }
}

fn yul_lit(lit: &YulLitKind) -> String {
    match lit {
        YulLitKind::Number(value) | YulLitKind::Hex(value) | YulLitKind::String(value) => {
            value.clone()
        }
        YulLitKind::Bool(value) => value.to_string(),
        YulLitKind::Error => "<error>".to_owned(),
    }
}

fn line(out: &mut String, indent: usize, text: &str) {
    let _ = writeln!(out, "{}{text}", "  ".repeat(indent));
}

fn escape_string(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out
}

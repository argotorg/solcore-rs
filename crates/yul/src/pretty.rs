use std::fmt::Write as _;

use crate::ast::{Case, Code, Data, DataValue, Expr, Inner, Literal, Object, Program, Stmt};

pub trait PrettyYul {
    fn to_yul_string(&self) -> String;
}

pub fn pretty_program(program: &Program) -> String {
    program.to_yul_string()
}

impl PrettyYul for Program {
    fn to_yul_string(&self) -> String {
        let mut out = String::new();
        for (index, object) in self.objects.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            write_object(&mut out, object, 0);
        }
        out
    }
}

impl PrettyYul for Object {
    fn to_yul_string(&self) -> String {
        let mut out = String::new();
        write_object(&mut out, self, 0);
        out
    }
}

impl PrettyYul for Code {
    fn to_yul_string(&self) -> String {
        let mut out = String::new();
        write_code(&mut out, self, 0);
        out
    }
}

impl PrettyYul for Stmt {
    fn to_yul_string(&self) -> String {
        let mut out = String::new();
        write_stmt(&mut out, self, 0);
        out
    }
}

impl PrettyYul for Expr {
    fn to_yul_string(&self) -> String {
        render_expr(self)
    }
}

fn write_object(out: &mut String, object: &Object, indent: usize) {
    line(
        out,
        indent,
        &format!("object \"{}\" {{", escape_string(&object.name)),
    );
    write_code(out, &object.code, indent + 1);
    for inner in &object.inners {
        match inner {
            Inner::Object(object) => write_object(out, object, indent + 1),
            Inner::Data(data) => write_data(out, data, indent + 1),
        }
    }
    line(out, indent, "}");
}

fn write_code(out: &mut String, code: &Code, indent: usize) {
    line(out, indent, "code {");
    for stmt in &code.stmts {
        write_stmt(out, stmt, indent + 1);
    }
    line(out, indent, "}");
}

fn write_data(out: &mut String, data: &Data, indent: usize) {
    let value = match &data.value {
        DataValue::Hex(value) => format!("hex\"{}\"", escape_hex_string(value)),
        DataValue::String(value) => format!("\"{}\"", escape_string(value)),
    };
    line(
        out,
        indent,
        &format!("data \"{}\" {value}", escape_string(&data.name)),
    );
}

fn write_stmt(out: &mut String, stmt: &Stmt, indent: usize) {
    match stmt {
        Stmt::Block(stmts) => {
            line(out, indent, "{");
            for stmt in stmts {
                write_stmt(out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        Stmt::Function {
            name,
            params,
            returns,
            body,
        } => {
            let returns = if returns.is_empty() {
                String::new()
            } else {
                format!(" -> {}", returns.join(", "))
            };
            line(
                out,
                indent,
                &format!("function {name}({}){returns} {{", params.join(", ")),
            );
            for stmt in body {
                write_stmt(out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        Stmt::Let { names, init } => match init {
            Some(init) => line(
                out,
                indent,
                &format!("let {} := {}", names.join(", "), render_expr(init)),
            ),
            None => line(out, indent, &format!("let {}", names.join(", "))),
        },
        Stmt::Assign { names, value } => {
            line(
                out,
                indent,
                &format!("{} := {}", names.join(", "), render_expr(value)),
            );
        }
        Stmt::If { cond, body } => {
            line(out, indent, &format!("if {} {{", render_expr(cond)));
            for stmt in body {
                write_stmt(out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        Stmt::Switch {
            expr: scrutinee,
            cases,
            default,
        } => {
            line(out, indent, &format!("switch {}", render_expr(scrutinee)));
            for case in cases {
                write_case(out, case, indent + 1);
            }
            if let Some(default) = default {
                line(out, indent + 1, "default {");
                for stmt in default {
                    write_stmt(out, stmt, indent + 2);
                }
                line(out, indent + 1, "}");
            }
        }
        Stmt::For {
            init,
            cond,
            post,
            body,
        } => {
            line(out, indent, "for {");
            for stmt in init {
                write_stmt(out, stmt, indent + 1);
            }
            line(out, indent, &format!("}} {} {{", render_expr(cond)));
            for stmt in post {
                write_stmt(out, stmt, indent + 1);
            }
            line(out, indent, "} {");
            for stmt in body {
                write_stmt(out, stmt, indent + 1);
            }
            line(out, indent, "}");
        }
        Stmt::Break => line(out, indent, "break"),
        Stmt::Continue => line(out, indent, "continue"),
        Stmt::Leave => line(out, indent, "leave"),
        Stmt::Comment(comment) => {
            line(
                out,
                indent,
                &format!("/* {} */", comment.replace("*/", "* /")),
            );
        }
        Stmt::Expr(value) => line(out, indent, &render_expr(value)),
    }
}

fn write_case(out: &mut String, case: &Case, indent: usize) {
    line(out, indent, &format!("case {} {{", lit(&case.lit)));
    for stmt in &case.body {
        write_stmt(out, stmt, indent + 1);
    }
    line(out, indent, "}");
}

fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Call { name, args } => {
            let args = args.iter().map(render_expr).collect::<Vec<_>>().join(", ");
            format!("{name}({args})")
        }
        Expr::Ident(name) => name.clone(),
        Expr::Lit(value) => lit(value),
    }
}

fn lit(lit: &Literal) -> String {
    match lit {
        Literal::Number(value) | Literal::Hex(value) => value.clone(),
        Literal::String(value) => format!("\"{}\"", escape_string(value)),
        Literal::Bool(true) => "true".to_owned(),
        Literal::Bool(false) => "false".to_owned(),
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
            ch if ch.is_control() => {
                let _ = write!(out, "\\x{:02x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out
}

fn escape_hex_string(value: &str) -> String {
    value.chars().filter(|ch| ch.is_ascii_hexdigit()).collect()
}

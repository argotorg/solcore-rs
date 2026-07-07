#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub objects: Vec<Object>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub name: String,
    pub code: Code,
    pub inners: Vec<Inner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inner {
    Object(Object),
    Data(Data),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Data {
    pub name: String,
    pub value: DataValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataValue {
    Hex(String),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    Block(Vec<Stmt>),
    Function {
        name: String,
        params: Vec<String>,
        returns: Vec<String>,
        body: Vec<Stmt>,
    },
    Let {
        names: Vec<String>,
        init: Option<Expr>,
    },
    Assign {
        names: Vec<String>,
        value: Expr,
    },
    If {
        cond: Expr,
        body: Vec<Stmt>,
    },
    Switch {
        expr: Expr,
        cases: Vec<Case>,
        default: Option<Vec<Stmt>>,
    },
    For {
        init: Vec<Stmt>,
        cond: Expr,
        post: Vec<Stmt>,
        body: Vec<Stmt>,
    },
    Break,
    Continue,
    Leave,
    Comment(String),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub lit: Literal,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Call { name: String, args: Vec<Expr> },
    Ident(String),
    Lit(Literal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Number(String),
    Hex(String),
    String(String),
    Bool(bool),
}

impl Program {
    pub fn single_object(object: Object) -> Self {
        Self {
            objects: vec![object],
        }
    }
}

impl Code {
    pub fn new(stmts: Vec<Stmt>) -> Self {
        Self { stmts }
    }
}

impl Expr {
    pub fn call(name: impl Into<String>, args: Vec<Expr>) -> Self {
        Self::Call {
            name: name.into(),
            args,
        }
    }

    pub fn ident(name: impl Into<String>) -> Self {
        Self::Ident(name.into())
    }

    pub fn number(value: impl Into<String>) -> Self {
        Self::Lit(Literal::Number(value.into()))
    }

    pub fn string(value: impl Into<String>) -> Self {
        Self::Lit(Literal::String(value.into()))
    }

    pub fn bool(value: bool) -> Self {
        Self::Lit(Literal::Bool(value))
    }
}

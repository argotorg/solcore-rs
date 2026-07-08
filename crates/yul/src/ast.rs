macro_rules! yul_name_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

yul_name_type!(FunctionName);
yul_name_type!(VarName);
yul_name_type!(ObjectName);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub objects: Vec<Object>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub name: ObjectName,
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
    pub name: ObjectName,
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
        name: FunctionName,
        params: Vec<VarName>,
        returns: Vec<VarName>,
        body: Vec<Stmt>,
    },
    Let {
        names: Vec<VarName>,
        init: Option<Expr>,
    },
    Assign {
        names: Vec<VarName>,
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
    Call { name: FunctionName, args: Vec<Expr> },
    Ident(VarName),
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
    pub fn call(name: impl Into<FunctionName>, args: Vec<Expr>) -> Self {
        Self::Call {
            name: name.into(),
            args,
        }
    }

    pub fn ident(name: impl Into<VarName>) -> Self {
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

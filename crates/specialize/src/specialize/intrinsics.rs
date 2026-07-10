use super::*;

pub(super) fn builtin_ctor_name(ctor: hir_nameres::BuiltinCtor) -> &'static str {
    match ctor {
        hir_nameres::BuiltinCtor::True => MonoBuiltinCtor::True.name(),
        hir_nameres::BuiltinCtor::False => MonoBuiltinCtor::False.name(),
        hir_nameres::BuiltinCtor::Unit => MonoBuiltinCtor::Unit.name(),
        hir_nameres::BuiltinCtor::Pair => MonoBuiltinCtor::Pair.name(),
        hir_nameres::BuiltinCtor::Inl => MonoBuiltinCtor::Inl.name(),
        hir_nameres::BuiltinCtor::Inr => MonoBuiltinCtor::Inr.name(),
    }
}

pub(super) fn builtin_name(kind: hir_nameres::BuiltinKind) -> &'static str {
    match kind {
        hir_nameres::BuiltinKind::Constructor(ctor) => builtin_ctor_name(ctor),
        hir_nameres::BuiltinKind::Function(function) => match function {
            hir_nameres::BuiltinFunction::Invoke => "invoke",
            hir_nameres::BuiltinFunction::PrimAddWord => "primAddWord",
            hir_nameres::BuiltinFunction::PrimEqWord => "primEqWord",
            hir_nameres::BuiltinFunction::WordToInteger => "wordToInteger",
            hir_nameres::BuiltinFunction::WordFromInteger => "wordFromInteger",
            hir_nameres::BuiltinFunction::IntegerAdd => "integerAdd",
            hir_nameres::BuiltinFunction::IntegerSub => "integerSub",
            hir_nameres::BuiltinFunction::IntegerMul => "integerMul",
            hir_nameres::BuiltinFunction::IntegerLt => "integerLt",
            hir_nameres::BuiltinFunction::IntegerEq => "integerEq",
        },
        hir_nameres::BuiltinKind::ClassMethod(method) => match method {
            hir_nameres::BuiltinClassMethod::InvokableInvoke => "invokable.invoke",
            hir_nameres::BuiltinClassMethod::IntFromInteger => "Int.fromInteger",
        },
        hir_nameres::BuiltinKind::Type(_) | hir_nameres::BuiltinKind::Class(_) => "<builtin>",
    }
}

pub(super) fn overloaded_operator_method(op: BinOp) -> Option<(&'static str, &'static str)> {
    match op {
        BinOp::Add => Some(("Add", "add")),
        BinOp::Sub => Some(("Sub", "sub")),
        BinOp::Gt => Some(("Ord", "gt")),
        _ => None,
    }
}

pub(super) fn plain_operator_function(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Lt => Some("lt"),
        BinOp::LtEq => Some("le"),
        BinOp::GtEq => Some("ge"),
        _ => None,
    }
}

pub(super) fn builtin_intrinsic(kind: hir_nameres::BuiltinKind) -> Option<MonoIntrinsic> {
    match kind {
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::PrimAddWord) => {
            Some(MonoIntrinsic::PrimAddWord)
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::PrimEqWord) => {
            Some(MonoIntrinsic::PrimEqWord)
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::WordToInteger) => {
            Some(MonoIntrinsic::WordToInteger)
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::WordFromInteger) => {
            Some(MonoIntrinsic::WordFromInteger)
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerAdd) => {
            Some(MonoIntrinsic::IntegerAdd)
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerSub) => {
            Some(MonoIntrinsic::IntegerSub)
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerMul) => {
            Some(MonoIntrinsic::IntegerMul)
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerLt) => {
            Some(MonoIntrinsic::IntegerLt)
        }
        hir_nameres::BuiltinKind::Function(hir_nameres::BuiltinFunction::IntegerEq) => {
            Some(MonoIntrinsic::IntegerEq)
        }
        _ => None,
    }
}

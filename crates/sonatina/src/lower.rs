use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use hir::{
    Db as HirDb,
    ast::function::{YulExpr, YulExprKind, YulLitKind, YulStmt, YulStmtKind},
};
use hull::{
    BinderId, Con, DebugBinder, Expr as HullExpr, ExprKind, Function as HullFunction,
    Object as HullObject, PatKind, Program as HullProgram, Stmt as HullStmt, StmtKind,
    Ty as HullTy, TyKind, origin::stmt_origin,
};
use smallvec::{SmallVec, smallvec};
use sonatina_ir::{
    BlockId, EmbedSymbol, I256, Immediate, InstId, Linkage, Module, Signature, Type, ValueId,
    builder::{FunctionBuilder, ModuleBuilder, ObjectBuilder, Variable},
    func_cursor::InstInserter,
    inst::{
        arith::{Add, Mul, Sar, Shl, Shr, Sub},
        cast::{PtrToInt, Trunc, Zext},
        cmp::{Eq, Gt, IsZero, Lt, Sgt, Slt},
        control_flow::{Br, Call, Jump, Return, Unreachable},
        data::{
            EnumAssertVariant, EnumExtract, EnumIsVariant, EnumMake, ExtractValue, InsertValue,
            SymAddr, SymSize, SymbolRef,
        },
        evm::{
            EvmAddMod, EvmAddress, EvmBalance, EvmBaseFee, EvmBlobBaseFee, EvmBlobHash,
            EvmBlockHash, EvmByte, EvmCall, EvmCallCode, EvmCallValue, EvmCalldataCopy,
            EvmCalldataLoad, EvmCalldataSize, EvmCaller, EvmChainId, EvmCodeCopy, EvmCodeSize,
            EvmCoinBase, EvmCreate, EvmCreate2, EvmDelegateCall, EvmExp, EvmExtCodeCopy,
            EvmExtCodeHash, EvmExtCodeSize, EvmGas, EvmGasLimit, EvmGasPrice, EvmInvalid,
            EvmKeccak256, EvmLog0, EvmLog1, EvmLog2, EvmLog3, EvmLog4, EvmMalloc, EvmMcopy,
            EvmMload, EvmMsize, EvmMstore, EvmMstore8, EvmMulMod, EvmNumber, EvmOrigin,
            EvmPrevRandao, EvmReturn, EvmReturnDataCopy, EvmReturnDataSize, EvmRevert, EvmSdiv,
            EvmSelfBalance, EvmSelfDestruct, EvmSignExtend, EvmSload, EvmSmod, EvmSstore,
            EvmStaticCall, EvmStop, EvmTimestamp, EvmTload, EvmTstore, EvmUdiv, EvmUmod,
            inst_set::EvmInstSet,
        },
        logic::{And, Clz, Not, Or, Xor},
    },
    isa::Isa,
    isa::evm::Evm,
    module::{FuncRef, ModuleCtx},
    types::{CompoundType, EnumReprHint, EnumVariantRef, VariantData},
};
use sonatina_triple::{Architecture, EvmVersion, OperatingSystem, TargetTriple, Vendor};
use sonatina_verifier::{VerificationLevel, VerifierConfig, verify_module};

use crate::TranslationError;

mod reachability;
mod tags;
mod values;

use reachability::Reachability;
use tags::{InstTag, SetId};
use values::ValueTrace;

type FunctionBuilderOf = FunctionBuilder<InstInserter>;

pub(crate) use tags::DebugTags;

use crate::locals::Frame;

/// With `stamp_origins`, instructions carry the origin of the Hull statement
/// that made them (`hull::origin::stmt_origin`); without it, no instruction has
/// an origin.
pub(super) fn translate_hull_program<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
    stamp_origins: bool,
) -> Result<Module, TranslationError> {
    translate_hull_program_tagged(db, program, stamp_origins).map(|(module, _)| module)
}

/// Like [`translate_hull_program`], with the visible source bindings around each
/// instruction that has an origin.
pub(super) fn translate_hull_program_tagged<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
    stamp_origins: bool,
) -> Result<(Module, DebugTags), TranslationError> {
    Translator::new(db, stamp_origins).translate(program)
}

pub(super) fn translate_hull_program_returning_runtime_entry<'db>(
    db: &'db dyn HirDb,
    program: &HullProgram<'db>,
    entry: &str,
) -> Result<Module, TranslationError> {
    let [object] = program.objects.as_slice() else {
        return Err(TranslationError::new(format!(
            "a runtime entry wrapper requires exactly one top-level object, found {}",
            program.objects.len()
        )));
    };
    if object.inners.len() != 1 {
        return Err(TranslationError::new(format!(
            "object `{}` must have exactly one runtime object",
            object.name
        )));
    }
    let mut translator = Translator::new(db, false);
    translator
        .returning_entries
        .insert("object0.inner0.init".to_owned(), entry.to_owned());
    translator.translate(program).map(|(module, _)| module)
}

fn evm_isa() -> Evm {
    Evm::new(TargetTriple::new(
        Architecture::Evm,
        Vendor::Ethereum,
        OperatingSystem::Evm(EvmVersion::Osaka),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TyKey {
    Word,
    Bool,
    Unit,
    Product(Box<TyKey>, Box<TyKey>),
    Sum(Box<TyKey>, Box<TyKey>),
    Named(String, Box<TyKey>),
    NamedRef(String),
    Function(Vec<TyKey>, Box<TyKey>),
}

impl TyKey {
    fn of(ty: &HullTy<'_>) -> Self {
        match &ty.kind {
            TyKind::Word => Self::Word,
            TyKind::Bool => Self::Bool,
            TyKind::Unit => Self::Unit,
            TyKind::Product(lhs, rhs) => {
                Self::Product(Box::new(Self::of(lhs)), Box::new(Self::of(rhs)))
            }
            TyKind::Sum(lhs, rhs) => Self::Sum(Box::new(Self::of(lhs)), Box::new(Self::of(rhs))),
            TyKind::Named { name, inner } => {
                Self::Named(name.as_str().to_owned(), Box::new(Self::of(inner)))
            }
            TyKind::NamedRef { name } => Self::NamedRef(name.as_str().to_owned()),
            TyKind::Function { params, ret } => Self::Function(
                params.iter().map(Self::of).collect(),
                Box::new(Self::of(ret)),
            ),
        }
    }
}

struct Translator<'db> {
    db: &'db dyn HirDb,
    builder: ModuleBuilder,
    isa: Evm,
    types: HashMap<TyKey, Type>,
    type_names: HashMap<TyKey, String>,
    functions: HashMap<String, FuncRef>,
    function_returns: HashMap<String, HullTy<'db>>,
    entries: HashMap<String, FuncRef>,
    returning_entries: HashMap<String, String>,
    section_objects: HashMap<String, String>,
    next_type: usize,
    next_yul_function: usize,
    /// Whether statements stamp the instructions they make with their origin.
    stamp_origins: bool,
    /// The visible source bindings around each stamped instruction.
    tags: DebugTags,
    /// The next number for a value label, which is unique in the module.
    next_label: u32,
}

impl<'db> Translator<'db> {
    fn new(db: &'db dyn HirDb, stamp_origins: bool) -> Self {
        let isa = evm_isa();
        let builder = ModuleBuilder::new(ModuleCtx::new(&isa));
        Self {
            stamp_origins,
            tags: DebugTags::default(),
            next_label: 0,
            db,
            builder,
            isa,
            types: HashMap::new(),
            type_names: HashMap::new(),
            functions: HashMap::new(),
            function_returns: HashMap::new(),
            entries: HashMap::new(),
            returning_entries: HashMap::new(),
            section_objects: HashMap::new(),
            next_type: 0,
            next_yul_function: 0,
        }
    }

    fn inst_set(&self) -> &'static EvmInstSet {
        self.isa.inst_set()
    }

    fn translate(
        mut self,
        program: &HullProgram<'db>,
    ) -> Result<(Module, DebugTags), TranslationError> {
        if program.objects.is_empty() {
            let entry_function = match program.entry_points.as_slice() {
                [] => None,
                [entry] => {
                    let function = program
                        .functions
                        .iter()
                        .find(|function| function.name == *entry)
                        .ok_or_else(|| {
                            TranslationError::new(format!(
                                "object-less Hull entry `{entry}` has no function body"
                            ))
                        })?;
                    if !function.args.is_empty() {
                        return Err(TranslationError::new(format!(
                            "object-less Hull entry `{entry}` must not take arguments"
                        )));
                    }
                    Some(function)
                }
                entries => {
                    return Err(TranslationError::new(format!(
                        "object-less Hull program must have at most one entry point, found {}",
                        entries.len()
                    )));
                }
            };
            self.section_objects
                .insert("root.runtime".to_owned(), "Output".to_owned());
            self.declare_code("root.runtime", &program.functions, &[], program.span)?;
            self.lower_code(
                "root.runtime",
                &program.functions,
                &[],
                program.span,
                entry_function,
            )?;
            let entry = self.entry("root.runtime")?;
            let mut object = ObjectBuilder::new("OutputDeploy");
            object.section("init").entry(entry);
            object.section("runtime").entry(entry);
            object
                .declare(&mut self.builder)
                .map_err(|err| TranslationError::new(format!("failed to declare object: {err}")))?;
        } else {
            for (index, object) in program.objects.iter().enumerate() {
                self.declare_object_code(object, &format!("object{index}"))?;
            }
            for (index, object) in program.objects.iter().enumerate() {
                self.lower_object_code(object, &format!("object{index}"))?;
            }
            for (index, object) in program.objects.iter().enumerate() {
                self.declare_object(object, &format!("object{index}"))?;
            }
        }

        let module = self.builder.build();
        let report = verify_module(&module, &VerifierConfig::for_level(VerificationLevel::Full));
        if report.has_errors() {
            return Err(TranslationError::new(format!(
                "Sonatina verification failed:\n{report}"
            )));
        }
        Ok((module, self.tags))
    }

    fn declare_object_code(
        &mut self,
        object: &HullObject<'db>,
        scope: &str,
    ) -> Result<(), TranslationError> {
        self.section_objects
            .insert(format!("{scope}.init"), object.name.as_str().to_owned());
        self.declare_code(
            &format!("{scope}.init"),
            &object.code.functions,
            &object.code.stmts,
            object.code.span,
        )?;
        for (index, inner) in object.inners.iter().enumerate() {
            self.declare_object_code(inner, &format!("{scope}.inner{index}"))?;
        }
        Ok(())
    }

    fn lower_object_code(
        &mut self,
        object: &HullObject<'db>,
        scope: &str,
    ) -> Result<(), TranslationError> {
        let section = format!("{scope}.init");
        let entry_function = if let Some(entry) = self.returning_entries.get(&section) {
            let function = object
                .code
                .functions
                .iter()
                .find(|function| function.name.as_str() == entry)
                .ok_or_else(|| {
                    TranslationError::new(format!(
                        "runtime entry `{entry}` has no function body in object `{}`",
                        object.name
                    ))
                })?;
            if !function.args.is_empty() || !self.lower_ty(&function.ret)?.is_integral() {
                return Err(TranslationError::new(format!(
                    "runtime entry `{}` must take no arguments and return a scalar word",
                    function.name
                )));
            }
            Some(function)
        } else {
            None
        };
        self.lower_code(
            &section,
            &object.code.functions,
            &object.code.stmts,
            object.code.span,
            entry_function,
        )?;
        for (index, inner) in object.inners.iter().enumerate() {
            self.lower_object_code(inner, &format!("{scope}.inner{index}"))?;
        }
        Ok(())
    }

    fn declare_object(
        &mut self,
        object: &HullObject<'db>,
        scope: &str,
    ) -> Result<(), TranslationError> {
        let init = self.entry(&format!("{scope}.init"))?;
        let mut builder = ObjectBuilder::new(object.name.as_str());
        builder.section("init").entry(init);
        if object.inners.is_empty() {
            builder.section("runtime").entry(init);
        } else {
            for (index, inner) in object.inners.iter().enumerate() {
                let inner_scope = format!("{scope}.inner{index}");
                let section = if index == 0 {
                    "runtime".to_owned()
                } else {
                    object_section_name(&inner_scope)
                };
                self.declare_inner_object_sections(&mut builder, inner, &inner_scope, &section)?;
                builder
                    .section("init")
                    .embed_local(section, inner.name.as_str());
            }
        }
        builder
            .declare(&mut self.builder)
            .map_err(|err| TranslationError::new(format!("failed to declare object: {err}")))
    }

    fn declare_inner_object_sections(
        &self,
        builder: &mut ObjectBuilder,
        object: &HullObject<'db>,
        scope: &str,
        section: &str,
    ) -> Result<(), TranslationError> {
        let entry = self.entry(&format!("{scope}.init"))?;
        builder.section(section).entry(entry);
        for (index, inner) in object.inners.iter().enumerate() {
            let inner_scope = format!("{scope}.inner{index}");
            let inner_section = object_section_name(&inner_scope);
            self.declare_inner_object_sections(builder, inner, &inner_scope, &inner_section)?;
            builder
                .section(section)
                .embed_local(inner_section, inner.name.as_str());
        }
        Ok(())
    }

    fn declare_code(
        &mut self,
        scope: &str,
        functions: &[HullFunction<'db>],
        _stmts: &[hull::Stmt<'db>],
        _span: hir::span::Span<'db>,
    ) -> Result<(), TranslationError> {
        for function in functions {
            let args = function
                .args
                .iter()
                .map(|arg| self.lower_ty(&arg.ty))
                .collect::<Result<Vec<_>, _>>()?;
            let ret = self.lower_ty(&function.ret)?;
            let symbol = function_symbol(scope, function.name.as_str());
            let signature = if ret == Type::Unit {
                Signature::new_unit(&symbol, Linkage::Private, &args)
            } else {
                Signature::new_single(&symbol, Linkage::Private, &args, ret)
            };
            let func = self.builder.declare_function(signature).map_err(|err| {
                TranslationError::new(format!("failed to declare `{symbol}`: {err}"))
            })?;
            self.functions
                .insert(key(scope, function.name.as_str()), func);
            self.function_returns
                .insert(key(scope, function.name.as_str()), function.ret.clone());
        }
        let entry_symbol = entry_symbol(scope);
        let entry = self
            .builder
            .declare_function(Signature::new_unit(&entry_symbol, Linkage::Public, &[]))
            .map_err(|err| TranslationError::new(format!("failed to declare entry: {err}")))?;
        self.entries.insert(scope.to_owned(), entry);
        Ok(())
    }

    fn lower_code(
        &mut self,
        scope: &str,
        functions: &[HullFunction<'db>],
        stmts: &[hull::Stmt<'db>],
        span: hir::span::Span<'db>,
        entry_function: Option<&HullFunction<'db>>,
    ) -> Result<(), TranslationError> {
        for function in functions {
            self.lower_function(scope, function)?;
        }
        self.lower_entry(scope, stmts, span, entry_function)
    }

    fn entry(&self, scope: &str) -> Result<FuncRef, TranslationError> {
        self.entries
            .get(scope)
            .copied()
            .ok_or_else(|| TranslationError::new(format!("missing section entry for `{scope}`")))
    }

    fn lower_ty(&mut self, ty: &HullTy<'db>) -> Result<Type, TranslationError> {
        let key = TyKey::of(ty);
        if let Some(existing) = self.types.get(&key) {
            return Ok(*existing);
        }
        let lowered = match &ty.kind {
            TyKind::Word => Type::I256,
            TyKind::Bool => Type::I1,
            TyKind::Unit => Type::Unit,
            TyKind::Product(lhs, rhs) => {
                let lhs = self.lower_ty(lhs)?;
                let rhs = self.lower_ty(rhs)?;
                let name = self.fresh_type_name("product", &key);
                self.builder.declare_struct_type(&name, &[lhs, rhs], false)
            }
            TyKind::Sum(lhs, rhs) if is_unit_ty(lhs) && is_unit_ty(rhs) => Type::I1,
            TyKind::Sum(lhs, rhs) => {
                let lhs_ty = self.lower_ty(lhs)?;
                let rhs_ty = self.lower_ty(rhs)?;
                let name = self.fresh_type_name("sum", &key);
                let variants = [
                    VariantData {
                        name: "inl".to_owned(),
                        explicit_discriminant: None,
                        fields: (!is_unit_ty(lhs)).then_some(lhs_ty).into_iter().collect(),
                    },
                    VariantData {
                        name: "inr".to_owned(),
                        explicit_discriminant: None,
                        fields: (!is_unit_ty(rhs)).then_some(rhs_ty).into_iter().collect(),
                    },
                ];
                self.builder
                    .declare_enum_type(&name, &variants, EnumReprHint::Default)
            }
            // Hull named types are transparent: their expressions are built from the
            // same product/sum values as the representation type.  Reusing that
            // structural type avoids introducing a nominal Sonatina mismatch.
            TyKind::Named { inner, .. } => self.lower_ty(inner)?,
            TyKind::NamedRef { .. } => Type::I256,
            TyKind::Function { params, ret } => {
                let params = params
                    .iter()
                    .map(|param| self.lower_ty(param))
                    .collect::<Result<Vec<_>, _>>()?;
                let ret = self.lower_ty(ret)?;
                let returns = (ret != Type::Unit)
                    .then_some(ret)
                    .into_iter()
                    .collect::<Vec<_>>();
                self.builder.declare_func_type(&params, &returns)
            }
        };
        self.types.insert(key, lowered);
        Ok(lowered)
    }

    fn fresh_type_name(&mut self, preferred: &str, key: &TyKey) -> String {
        if let Some(name) = self.type_names.get(key) {
            return name.clone();
        }
        let stem = sanitize(preferred);
        let name = format!("solcore_{stem}_{}", self.next_type);
        self.next_type += 1;
        self.type_names.insert(key.clone(), name.clone());
        name
    }

    fn lower_function(
        &mut self,
        scope: &str,
        function: &HullFunction<'db>,
    ) -> Result<(), TranslationError> {
        let func_ref = self
            .functions
            .get(&key(scope, function.name.as_str()))
            .copied()
            .ok_or_else(|| {
                TranslationError::new(format!(
                    "missing declaration for `{}`",
                    function.name.as_str()
                ))
            })?;
        let mut lowerer =
            FunctionLowerer::new(self, scope, func_ref, function.ret.clone(), Some(function));
        for (index, arg) in function.args.iter().enumerate() {
            let value = lowerer.fb.func.arg_values[index];
            lowerer.bind_parameter(arg.name.as_str(), &arg.ty, value)?;
            lowerer.add_binder(arg.binder, arg.name.as_str());
        }
        let terminated = lowerer.lower_stmts(&function.body)?;
        lowerer.finish(terminated)
    }

    fn lower_entry(
        &mut self,
        scope: &str,
        stmts: &[hull::Stmt<'db>],
        span: hir::span::Span<'db>,
        entry_function: Option<&HullFunction<'db>>,
    ) -> Result<(), TranslationError> {
        let func_ref = self.entry(scope)?;
        let unit = HullTy::unit(span);
        let mut lowerer = FunctionLowerer::new(self, scope, func_ref, unit, None);
        let mut terminated = lowerer.lower_stmts(stmts)?;
        if !terminated && let Some(main) = entry_function {
            let callee = lowerer
                .module
                .functions
                .get(&key(scope, main.name.as_str()))
                .copied()
                .ok_or_else(|| TranslationError::new("missing entry declaration"))?;
            let ret = lowerer.module.lower_ty(&main.ret)?;
            let call = Call::new(lowerer.module.inst_set(), callee, SmallVec::new());
            if ret == Type::Unit {
                lowerer.fb.insert_inst_no_result(call);
            } else {
                let result = lowerer.fb.insert_inst(call, ret);
                if ret.is_integral() {
                    let result = lowerer.coerce(result, Type::I256)?;
                    let zero = lowerer.fb.make_imm_value(I256::zero());
                    let size = lowerer.fb.make_imm_value(I256::from(32u8));
                    lowerer.fb.insert_inst_no_result(EvmMstore::new(
                        lowerer.module.inst_set(),
                        zero,
                        result,
                    ));
                    lowerer.fb.insert_inst_no_result(EvmReturn::new(
                        lowerer.module.inst_set(),
                        zero,
                        size,
                    ));
                    terminated = true;
                }
            }
        }
        if !terminated {
            // Object sections are entered by the EVM, not an internal call.
            // Falling through must halt rather than pop a return address.
            lowerer
                .fb
                .insert_inst_no_result(EvmStop::new(lowerer.module.inst_set()));
        }
        lowerer.finish(true)
    }
}

#[derive(Clone)]
struct Binding<'db> {
    var: Variable,
    ty: Type,
    hull_ty: HullTy<'db>,
}

enum BuiltinOutcome {
    Value(ValueId),
    Unit,
    Terminated,
}

enum YulExprOutcome {
    Values(SmallVec<[ValueId; 2]>),
    Terminated,
}

#[derive(Debug, Clone, Copy)]
struct YulFunctionDecl {
    func: FuncRef,
    params: usize,
    returns: usize,
}

struct FunctionLowerer<'a, 'db> {
    module: &'a mut Translator<'db>,
    scope: String,
    fb: FunctionBuilder<InstInserter>,
    reachability: Reachability,
    scopes: Vec<HashMap<String, Binding<'db>>>,
    ret: HullTy<'db>,
    break_targets: Vec<BlockId>,
    continue_targets: Vec<BlockId>,
    yul_functions: Vec<HashMap<String, YulFunctionDecl>>,
    yul_returns: Option<Vec<Variable>>,
    /// The Hull function being lowered, or `None` for object-level code. Inline
    /// Yul functions keep the Hull function that contains their `assembly`
    /// block. Statements are stamped by `hull::origin::stmt_origin`, which
    /// needs it to apply the generated-code rule.
    function: Option<&'a HullFunction<'db>>,
    /// Instructions a finished statement has taken, whether it stamped them or
    /// left them unmapped.
    claimed: HashSet<InstId>,
    func_ref: FuncRef,
    /// The source bindings declared in each open scope, as binder ids in the
    /// order they became visible. Frames follow [`Self::scopes`], and the body
    /// and the step of a loop have frames of their own.
    binder_frames: Vec<Vec<u32>>,
    /// The set of all bindings in `binder_frames`.
    visible: SetId,
    /// Which value held each binding, as lowering saw it (see `values`).
    trace: ValueTrace,
}

impl<'a, 'db> FunctionLowerer<'a, 'db> {
    fn new(
        module: &'a mut Translator<'db>,
        scope: &str,
        func_ref: FuncRef,
        ret: HullTy<'db>,
        function: Option<&'a HullFunction<'db>>,
    ) -> Self {
        let mut fb = module.builder.func_builder::<InstInserter>(func_ref);
        let entry = fb.append_block();
        fb.switch_to_block(entry);
        let visible = module.tags.intern(&[]);
        Self {
            module,
            scope: scope.to_owned(),
            fb,
            reachability: Reachability::new(entry),
            scopes: vec![HashMap::new()],
            ret,
            break_targets: Vec::new(),
            continue_targets: Vec::new(),
            yul_functions: Vec::new(),
            yul_returns: None,
            function,
            claimed: HashSet::new(),
            func_ref,
            binder_frames: vec![Vec::new()],
            visible,
            trace: ValueTrace::default(),
        }
    }

    fn new_yul(
        module: &'a mut Translator<'db>,
        scope: &str,
        func_ref: FuncRef,
        span: hir::span::Span<'db>,
        yul_functions: Vec<HashMap<String, YulFunctionDecl>>,
        function: Option<&'a HullFunction<'db>>,
    ) -> Self {
        let mut lowerer = Self::new(module, scope, func_ref, HullTy::unit(span), function);
        lowerer.yul_functions = yul_functions;
        lowerer.yul_returns = Some(Vec::new());
        lowerer
    }

    fn finish(mut self, terminated: bool) -> Result<(), TranslationError> {
        if !terminated {
            let ret = self.module.lower_ty(&self.ret)?;
            if ret == Type::Unit {
                self.fb
                    .insert_inst_no_result(Return::new_unit(self.module.inst_set()));
            } else {
                let value = zero_for_type(&mut self.fb, self.module.inst_set(), ret);
                self.fb
                    .insert_inst_no_result(Return::new_single(self.module.inst_set(), value));
            }
        }
        self.label_values();
        self.fb.seal_all();
        self.fb.finish();
        Ok(())
    }

    fn finish_yul(mut self, terminated: bool) -> Result<(), TranslationError> {
        if !terminated {
            self.emit_yul_return()?;
        }
        self.label_values();
        self.fb.seal_all();
        self.fb.finish();
        Ok(())
    }

    /// Labels the values that hold source bindings, and records the labels in the
    /// tags of the function's instructions. Runs before the SSA builder seals its
    /// blocks, so that a phi it then removes as trivial hands its labels on. It
    /// adds no instruction and changes no operand.
    fn label_values(&mut self) {
        if !self.module.stamp_origins {
            return;
        }
        let module = &mut *self.module;
        self.trace.label(
            &mut self.fb,
            &mut module.tags,
            self.func_ref,
            &mut module.next_label,
        );
    }

    /// `FunctionBuilder::def_var`, which also notes the value for the debug
    /// labels.
    fn def_var(&mut self, var: Variable, value: ValueId) {
        self.fb.def_var(var, value);
        self.note_variable(var, value);
    }

    /// `FunctionBuilder::use_var`, which also notes the value for the debug
    /// labels. The value is what the builder returned: nothing is read that the
    /// program did not read.
    fn use_var(&mut self, var: Variable) -> ValueId {
        let value = self.fb.use_var(var);
        self.note_variable(var, value);
        value
    }

    fn note_variable(&mut self, var: Variable, value: ValueId) {
        if !self.module.stamp_origins {
            return;
        }
        if let Some(block) = self.fb.current_block() {
            self.trace
                .note(block, var, self.fb.func.dfg.num_insts(), value);
        }
    }

    fn emit_yul_return(&mut self) -> Result<(), TranslationError> {
        let returns = self
            .yul_returns
            .clone()
            .ok_or_else(|| TranslationError::new("inline Yul `leave` outside a Yul function"))?;
        let values = returns
            .iter()
            .map(|var| self.use_var(*var))
            .collect::<SmallVec<[ValueId; 2]>>();
        self.fb.insert_return_values(&values);
        Ok(())
    }

    fn bind_parameter(
        &mut self,
        name: &str,
        hull_ty: &HullTy<'db>,
        value: ValueId,
    ) -> Result<(), TranslationError> {
        let ty = self.module.lower_ty(hull_ty)?;
        let var = self.fb.declare_var(ty);
        self.def_var(var, value);
        self.insert_binding(
            name,
            Binding {
                var,
                ty,
                hull_ty: hull_ty.clone(),
            },
        );
        Ok(())
    }

    fn declare_binding(
        &mut self,
        name: &str,
        hull_ty: &HullTy<'db>,
    ) -> Result<Binding<'db>, TranslationError> {
        let ty = self.module.lower_ty(hull_ty)?;
        let var = self.fb.declare_var(ty);
        let initial = zero_for_type(&mut self.fb, self.module.inst_set(), ty);
        self.def_var(var, initial);
        let binding = Binding {
            var,
            ty,
            hull_ty: hull_ty.clone(),
        };
        self.insert_binding(name, binding.clone());
        Ok(binding)
    }

    fn bind_value(
        &mut self,
        name: &str,
        hull_ty: &HullTy<'db>,
        value: ValueId,
    ) -> Result<(), TranslationError> {
        let binding = self.declare_binding(name, hull_ty)?;
        let value = self.coerce(value, binding.ty)?;
        self.def_var(binding.var, value);
        Ok(())
    }

    fn insert_binding(&mut self, name: &str, binding: Binding<'db>) {
        self.scopes
            .last_mut()
            .expect("scope stack is never empty")
            .insert(name.to_owned(), binding);
    }

    fn lookup(&self, name: &str) -> Result<Binding<'db>, TranslationError> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .ok_or_else(|| TranslationError::new(format!("undefined Hull variable `{name}`")))
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.push_frame();
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.pop_frame();
    }

    fn push_frame(&mut self) {
        self.binder_frames.push(Vec::new());
    }

    /// Leaves a scope of source bindings. The bindings that were declared in it
    /// are no longer visible after the last instruction made so far.
    fn pop_frame(&mut self) {
        let declared = self
            .binder_frames
            .pop()
            .is_some_and(|frame| !frame.is_empty());
        if declared {
            self.refresh_visible();
            self.retag_last();
        }
    }

    fn refresh_visible(&mut self) {
        let set: Vec<u32> = self.binder_frames.iter().flatten().copied().collect();
        self.visible = self.module.tags.intern(&set);
    }

    /// Makes the source binding visible, in the innermost scope, from here on.
    /// Returns whether there is one. Nothing is tracked unless instructions carry
    /// origins.
    fn add_binder(&mut self, binder: DebugBinder, name: &str) -> bool {
        let Some(BinderId(id)) = binder.0 else {
            return false;
        };
        if !self.module.stamp_origins {
            return false;
        }
        // A binding of one word can have a value label; others have none.
        if let Ok(binding) = self.lookup(name)
            && binding.ty.is_integral()
        {
            self.trace.bind(id, binding.var);
        }
        self.binder_frames
            .last_mut()
            .expect("binder frames are never empty")
            .push(id);
        self.refresh_visible();
        true
    }

    /// States that the instruction made last in the current block completes with
    /// the bindings that are visible now. The change in what is visible executes
    /// nothing, so the state after that instruction is the state at this point.
    /// A block with no instruction yet, or one that ends in a terminator or in
    /// an instruction without an origin, is left alone: the change then shows from
    /// the next instruction on.
    fn retag_last(&mut self) {
        if !self.module.stamp_origins {
            return;
        }
        let Some(inst) = self.fb.last_inst() else {
            return;
        };
        if self.fb.is_terminator(inst) {
            return;
        }
        if let Some(tag) = self.module.tags.insts.get_mut(&(self.func_ref, inst)) {
            tag.after = self.visible;
        }
    }

    fn lower_stmts(&mut self, stmts: &[HullStmt<'db>]) -> Result<bool, TranslationError> {
        for stmt in stmts {
            if self.lower_stmt(stmt)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Lowers one statement, then stamps the instructions it created with the
    /// statement's origin. Nested statements are lowered and stamped first, so
    /// an enclosing statement only claims what its children did not (loop
    /// headers, branches, join code).
    fn lower_stmt(&mut self, stmt: &HullStmt<'db>) -> Result<bool, TranslationError> {
        let start = self.visible;
        let watermark = self.fb.func.dfg.num_insts();
        let result = self.lower_stmt_inner(stmt);
        let leaves_scopes = matches!(stmt.kind, StmtKind::Break | StmtKind::Continue);
        self.stamp_insts_since(watermark, stmt.span, start, leaves_scopes);
        if matches!(result, Ok(false)) {
            let (binds, name) = match &stmt.kind {
                StmtKind::Let { binds, name, .. } => (*binds, name.as_str()),
                StmtKind::Assign { binds, lhs, .. } => {
                    (*binds, assignment_path(lhs).map_or("", |(name, _)| name))
                }
                _ => (DebugBinder::default(), ""),
            };
            if self.add_binder(binds, name) {
                self.retag_last();
            }
        }
        result
    }

    /// Gives every instruction created since `watermark` that no nested
    /// statement has taken the origin of a statement at `span`. A zero-width
    /// span has no origin: it takes the instructions and leaves them unmapped,
    /// so an enclosing statement does not claim them. Code that no statement
    /// made (the implicit return, the fall-through stop, phis made when blocks
    /// are sealed) is never stamped and stays unmapped.
    fn stamp_insts_since(
        &mut self,
        watermark: usize,
        span: hir::span::Span<'db>,
        visible: SetId,
        leaves_scopes: bool,
    ) {
        let end = self.fb.func.dfg.num_insts();
        if !self.module.stamp_origins || end == watermark {
            return;
        }
        let origin = stmt_origin(self.function, span)
            .resolve(self.module.db)
            .map(|origin| Arc::<str>::from(origin.encode()));
        for index in watermark..end {
            let inst = InstId(index as u32);
            if !self.claimed.insert(inst) {
                continue;
            }
            if let Some(origin) = &origin {
                self.fb
                    .func
                    .set_inst_frontend_origin(inst, Arc::clone(origin));
                let dfg = &self.fb.func.dfg;
                let frame = if dfg.is_call(inst) {
                    Frame::Call
                } else if dfg.is_return(inst) {
                    Frame::Return
                } else if leaves_scopes {
                    Frame::Jump
                } else {
                    Frame::Plain
                };
                self.module.tags.insts.insert(
                    (self.func_ref, inst),
                    InstTag {
                        before: visible,
                        after: visible,
                        frame,
                        before_labels: Vec::new(),
                        after_labels: Vec::new(),
                    },
                );
            }
        }
    }

    fn lower_stmt_inner(&mut self, stmt: &HullStmt<'db>) -> Result<bool, TranslationError> {
        match &stmt.kind {
            StmtKind::Let { name, ty, .. } => {
                self.declare_binding(name.as_str(), ty)?;
                Ok(false)
            }
            StmtKind::Assign { lhs, rhs, .. } => {
                let value = self.lower_expr(rhs)?;
                self.assign(lhs, value)?;
                Ok(false)
            }
            StmtKind::Expr(expr) => {
                if let ExprKind::Call { callee, args } = &expr.kind
                    && !self
                        .module
                        .functions
                        .contains_key(&key(&self.scope, callee.as_str()))
                    && is_terminal_evm_builtin(callee.as_str())
                {
                    let values = args
                        .iter()
                        .map(|arg| self.lower_expr(arg))
                        .collect::<Result<Vec<_>, _>>()?;
                    return Ok(matches!(
                        self.lower_evm_builtin(callee.as_str(), &values)?,
                        BuiltinOutcome::Terminated
                    ));
                }
                let _ = self.lower_expr(expr)?;
                Ok(false)
            }
            StmtKind::Return(expr) => {
                let value = self.lower_expr(expr)?;
                let ret_ty = self.module.lower_ty(&self.ret)?;
                if ret_ty == Type::Unit {
                    self.fb
                        .insert_inst_no_result(Return::new_unit(self.module.inst_set()));
                } else {
                    let value = self.coerce(value, ret_ty)?;
                    self.fb
                        .insert_inst_no_result(Return::new_single(self.module.inst_set(), value));
                }
                Ok(true)
            }
            StmtKind::Block(body) => {
                self.push_scope();
                let terminated = self.lower_stmts(body)?;
                self.pop_scope();
                Ok(terminated)
            }
            StmtKind::For {
                init,
                cond,
                post,
                body,
            } => self.lower_for(init, cond, post, body),
            StmtKind::Break => {
                let target = self
                    .break_targets
                    .last()
                    .copied()
                    .ok_or_else(|| TranslationError::new("break outside loop"))?;
                self.emit_jump(target);
                Ok(true)
            }
            StmtKind::Continue => {
                let target = self
                    .continue_targets
                    .last()
                    .copied()
                    .ok_or_else(|| TranslationError::new("continue outside loop"))?;
                self.emit_jump(target);
                Ok(true)
            }
            StmtKind::Match {
                target,
                scrutinee,
                alts,
                condition,
            } => self.lower_match(target, scrutinee, alts, condition.0),
            StmtKind::Assembly(stmts) => self.lower_yul_stmts(stmts),
            StmtKind::Revert(message) => {
                let bytes = message.as_bytes();
                if bytes.len() > 32 {
                    return Err(TranslationError::new(
                        "literal revert message exceeds one EVM word",
                    ));
                }
                let zero = self.fb.make_imm_value(I256::zero());
                let mut word = [0u8; 32];
                word[..bytes.len()].copy_from_slice(bytes);
                let message = self.fb.make_imm_value(I256::from_be_bytes(&word));
                let size = self.fb.make_imm_value(I256::from(bytes.len()));
                self.fb.insert_inst_no_result(EvmMstore::new(
                    self.module.inst_set(),
                    zero,
                    message,
                ));
                self.fb
                    .insert_inst_no_result(EvmRevert::new(self.module.inst_set(), zero, size));
                Ok(true)
            }
            StmtKind::Comment(_) => Ok(false),
        }
    }

    fn lower_expr(&mut self, expr: &HullExpr<'db>) -> Result<ValueId, TranslationError> {
        match &expr.kind {
            ExprKind::Word(value) => self.word_value(value),
            ExprKind::Bool(value) => Ok(self.fb.make_imm_value(*value)),
            ExprKind::Unit => Ok(self.fb.func.dfg.make_zero_const(Type::Unit)),
            ExprKind::Var(name) => {
                let binding = self.lookup(name.as_str())?;
                Ok(self.use_var(binding.var))
            }
            ExprKind::Pair(lhs, rhs) => {
                let ty = self.module.lower_ty(&expr.ty)?;
                let (lhs_ty, rhs_ty) = product_parts(&expr.ty)?;
                let lhs = self.lower_expr(lhs)?;
                let lhs_target = self.module.lower_ty(lhs_ty)?;
                let lhs = self.coerce(lhs, lhs_target)?;
                let rhs = self.lower_expr(rhs)?;
                let rhs_target = self.module.lower_ty(rhs_ty)?;
                let rhs = self.coerce(rhs, rhs_target)?;
                let mut value = self.fb.func.dfg.make_zero_const(ty);
                let zero = self.index_value(0);
                value = self.fb.insert_inst(
                    InsertValue::new(self.module.inst_set(), value, zero, lhs),
                    ty,
                );
                let one = self.index_value(1);
                Ok(self.fb.insert_inst(
                    InsertValue::new(self.module.inst_set(), value, one, rhs),
                    ty,
                ))
            }
            ExprKind::Fst(inner) => {
                let value = self.lower_expr(inner)?;
                let (lhs, _) = product_parts(&inner.ty)?;
                let ty = self.module.lower_ty(lhs)?;
                let index = self.index_value(0);
                Ok(self
                    .fb
                    .insert_inst(ExtractValue::new(self.module.inst_set(), value, index), ty))
            }
            ExprKind::Snd(inner) => {
                let value = self.lower_expr(inner)?;
                let (_, rhs) = product_parts(&inner.ty)?;
                let ty = self.module.lower_ty(rhs)?;
                let index = self.index_value(1);
                Ok(self
                    .fb
                    .insert_inst(ExtractValue::new(self.module.inst_set(), value, index), ty))
            }
            ExprKind::Inl { target, value } => {
                let value = self.lower_expr(value)?;
                self.lower_variant(target, 0, value)
            }
            ExprKind::Inr { target, value } => {
                let value = self.lower_expr(value)?;
                self.lower_variant(target, 1, value)
            }
            ExprKind::InK {
                index,
                target,
                value,
            } => {
                let value = self.lower_expr(value)?;
                self.lower_injection(target, *index, value)
            }
            ExprKind::Call { callee, args } => {
                let values = args
                    .iter()
                    .map(|arg| self.lower_expr(arg))
                    .collect::<Result<SmallVec<[ValueId; 8]>, _>>()?;
                if let Some(func) = self
                    .module
                    .functions
                    .get(&key(&self.scope, callee.as_str()))
                    .copied()
                {
                    let ret = self.module.lower_ty(&expr.ty)?;
                    let call = Call::new(self.module.inst_set(), func, values);
                    if ret == Type::Unit {
                        self.fb.insert_inst_no_result(call);
                        Ok(self.fb.func.dfg.make_zero_const(Type::Unit))
                    } else {
                        Ok(self.fb.insert_inst(call, ret))
                    }
                } else {
                    self.lower_builtin_call(callee.as_str(), &values, &expr.ty)
                }
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
                ..
            } => self.lower_if_expr(cond, then_expr, else_expr, &expr.ty),
        }
    }

    fn assign(&mut self, lhs: &HullExpr<'db>, value: ValueId) -> Result<(), TranslationError> {
        let (name, path) = assignment_path(lhs)?;
        let binding = self.lookup(name)?;
        let value = if path.is_empty() {
            self.coerce(value, binding.ty)?
        } else {
            let root = self.use_var(binding.var);
            self.insert_at_path(root, &binding.hull_ty, &path, value)?
        };
        self.def_var(binding.var, value);
        Ok(())
    }

    fn insert_at_path(
        &mut self,
        aggregate: ValueId,
        aggregate_ty: &HullTy<'db>,
        path: &[usize],
        value: ValueId,
    ) -> Result<ValueId, TranslationError> {
        let Some((&head, tail)) = path.split_first() else {
            let target = self.module.lower_ty(aggregate_ty)?;
            return self.coerce(value, target);
        };
        let (lhs, rhs) = product_parts(aggregate_ty)?;
        let field_ty = if head == 0 { lhs } else { rhs };
        let field = if tail.is_empty() {
            let target = self.module.lower_ty(field_ty)?;
            self.coerce(value, target)?
        } else {
            let index = self.index_value(head);
            let extracted = self.fb.insert_inst(
                ExtractValue::new(self.module.inst_set(), aggregate, index),
                self.module.lower_ty(field_ty)?,
            );
            self.insert_at_path(extracted, field_ty, tail, value)?
        };
        let index = self.index_value(head);
        Ok(self.fb.insert_inst(
            InsertValue::new(self.module.inst_set(), aggregate, index, field),
            self.module.lower_ty(aggregate_ty)?,
        ))
    }

    fn lower_if_expr(
        &mut self,
        cond: &HullExpr<'db>,
        then_expr: &HullExpr<'db>,
        else_expr: &HullExpr<'db>,
        result_ty: &HullTy<'db>,
    ) -> Result<ValueId, TranslationError> {
        let ty = self.module.lower_ty(result_ty)?;
        let result = self.fb.declare_var(ty);
        let initial = zero_for_type(&mut self.fb, self.module.inst_set(), ty);
        self.def_var(result, initial);
        let then_block = self.fb.append_block();
        let else_block = self.fb.append_block();
        let done = self.fb.append_block();
        let cond = self.lower_condition(cond)?;
        self.emit_branch(cond, then_block, else_block);
        self.fb.switch_to_block(then_block);
        let then_value = self.lower_expr(then_expr)?;
        let then_value = self.coerce(then_value, ty)?;
        self.def_var(result, then_value);
        self.emit_jump(done);
        self.fb.switch_to_block(else_block);
        let else_value = self.lower_expr(else_expr)?;
        let else_value = self.coerce(else_value, ty)?;
        self.def_var(result, else_value);
        self.emit_jump(done);
        self.fb.switch_to_block(done);
        Ok(self.use_var(result))
    }

    /// Record edges at their single emission point so continuation reachability
    /// stays current without rescanning the function under construction.
    fn emit_jump(&mut self, target: BlockId) {
        let source = self
            .fb
            .current_block()
            .expect("lowering has a current block");
        self.reachability.add_edge(source, target);
        self.fb
            .insert_inst_no_result(Jump::new(self.module.inst_set(), target));
    }

    fn emit_branch(&mut self, cond: ValueId, then_block: BlockId, else_block: BlockId) {
        let source = self
            .fb
            .current_block()
            .expect("lowering has a current block");
        self.reachability.add_edge(source, then_block);
        self.reachability.add_edge(source, else_block);
        self.fb.insert_inst_no_result(Br::new(
            self.module.inst_set(),
            cond,
            then_block,
            else_block,
        ));
    }

    /// Enter a continuation only when a path from the function entry reaches
    /// it. A terminating body can still have `continue` edges, while a merge
    /// block with predecessors can itself be unreachable. Neither syntactic
    /// fallthrough nor predecessor count alone describes that distinction.
    fn switch_to_reachable_block(&mut self, block: BlockId) -> bool {
        let reachable = self.reachability.contains(block);
        self.fb.switch_to_block(block);
        if !reachable {
            // Keep the allocated block well formed without constructing SSA
            // uses for locals that have no reaching definition on this path.
            self.fb
                .insert_inst_no_result(Unreachable::new_unchecked(self.module.inst_set()));
        }
        reachable
    }

    fn lower_for(
        &mut self,
        init: &[HullStmt<'db>],
        cond: &HullExpr<'db>,
        post: &[HullStmt<'db>],
        body: &[HullStmt<'db>],
    ) -> Result<bool, TranslationError> {
        self.push_scope();
        if self.lower_stmts(init)? {
            self.pop_scope();
            return Ok(true);
        }
        let header = self.fb.append_block();
        let body_block = self.fb.append_block();
        let post_block = self.fb.append_block();
        let done = self.fb.append_block();
        self.emit_jump(header);
        self.fb.switch_to_block(header);
        let cond = self.lower_condition(cond)?;
        self.emit_branch(cond, body_block, done);
        self.fb.switch_to_block(body_block);
        self.break_targets.push(done);
        self.continue_targets.push(post_block);
        // The body and the step are scopes of the source, though they share the
        // loop's scope here.
        self.push_frame();
        let body_terminated = self.lower_stmts(body)?;
        self.pop_frame();
        self.continue_targets.pop();
        self.break_targets.pop();
        if !body_terminated {
            self.emit_jump(post_block);
        }
        if self.switch_to_reachable_block(post_block) {
            self.push_frame();
            let post_terminated = self.lower_stmts(post)?;
            self.pop_frame();
            if !post_terminated {
                self.emit_jump(header);
            }
        }
        self.fb.switch_to_block(done);
        self.pop_scope();
        Ok(false)
    }

    /// Lowers a `match`. For a statement that stands for a source `if`, the
    /// span of its `condition` is given: the scrutinee and the pattern tests
    /// belong to the condition's text, and the jumps that leave an arm to no
    /// text, because the range of the whole `if` also covers the arms and a jump
    /// after an arm must not show it again.
    fn lower_match(
        &mut self,
        target: &HullTy<'db>,
        scrutinee: &HullExpr<'db>,
        alts: &[hull::Alt<'db>],
        condition: Option<hir::span::Span<'db>>,
    ) -> Result<bool, TranslationError> {
        let start = self.visible;
        let watermark = self.fb.func.dfg.num_insts();
        let value = self.lower_expr(scrutinee)?;
        if let Some(condition) = condition {
            self.stamp_insts_since(watermark, condition, start, false);
        }
        let done = self.fb.append_block();
        let mut exhaustive = false;
        for alt in alts {
            let arm = self.fb.append_block();
            let catch_all = matches!(&alt.pat.kind, PatKind::Wildcard | PatKind::Var(_));
            let next = (!catch_all).then(|| self.fb.append_block());
            if catch_all {
                exhaustive = true;
                self.emit_jump(arm);
            } else {
                match &alt.pat.kind {
                    PatKind::Wildcard | PatKind::Var(_) => unreachable!(),
                    pat => {
                        let watermark = self.fb.func.dfg.num_insts();
                        let cond = self.pattern_condition(value, target, pat)?;
                        self.emit_branch(cond, arm, next.expect("non-catch-all has a next block"));
                        if let Some(condition) = condition {
                            self.stamp_insts_since(watermark, condition, start, false);
                        }
                    }
                }
            }
            self.fb.switch_to_block(arm);
            self.push_scope();
            let (binder_ty, binder_value) = self.pattern_payload(value, target, &alt.pat.kind)?;
            self.bind_value(alt.binder.as_str(), &binder_ty, binder_value)?;
            if let PatKind::Var(name) = &alt.pat.kind {
                self.bind_value(name.as_str(), target, value)?;
            }
            let terminated = self.lower_stmts(&alt.body)?;
            self.pop_scope();
            if !terminated {
                self.emit_jump_to_done(done, condition.is_some());
            }
            if let Some(next) = next {
                self.fb.switch_to_block(next);
            } else {
                break;
            }
        }
        if !exhaustive {
            self.emit_jump_to_done(done, condition.is_some());
        }
        Ok(!self.switch_to_reachable_block(done))
    }

    /// Emits the jump that ends an arm of a `match`. With `unmapped`, no
    /// enclosing statement may claim it, so it stays without an origin.
    fn emit_jump_to_done(&mut self, done: BlockId, unmapped: bool) {
        let watermark = self.fb.func.dfg.num_insts();
        self.emit_jump(done);
        if unmapped {
            for index in watermark..self.fb.func.dfg.num_insts() {
                self.claimed.insert(InstId(index as u32));
            }
        }
    }

    fn pattern_condition(
        &mut self,
        value: ValueId,
        target: &HullTy<'db>,
        pat: &PatKind,
    ) -> Result<ValueId, TranslationError> {
        match pat {
            PatKind::Con(Con::Inl) => self.is_variant(value, target, 0),
            PatKind::Con(Con::Inr) => self.is_variant(value, target, 1),
            PatKind::Con(Con::InK(index)) => self.in_k_condition(value, target, *index),
            PatKind::IntLit(text) => {
                let rhs = self.word_value(text)?;
                Ok(self
                    .fb
                    .insert_inst(Eq::new(self.module.inst_set(), value, rhs), Type::I1))
            }
            PatKind::Wildcard | PatKind::Var(_) => Ok(self.fb.make_imm_value(true)),
        }
    }

    fn pattern_payload(
        &mut self,
        value: ValueId,
        target: &HullTy<'db>,
        pat: &PatKind,
    ) -> Result<(HullTy<'db>, ValueId), TranslationError> {
        match pat {
            PatKind::Con(Con::Inl) => self.extract_variant(value, target, 0),
            PatKind::Con(Con::Inr) => self.extract_variant(value, target, 1),
            PatKind::Con(Con::InK(index)) => self.extract_in_k(value, target, *index),
            PatKind::Wildcard | PatKind::Var(_) | PatKind::IntLit(_) => Ok((target.clone(), value)),
        }
    }

    fn is_variant(
        &mut self,
        value: ValueId,
        target: &HullTy<'db>,
        index: u32,
    ) -> Result<ValueId, TranslationError> {
        if is_bool_like(target) {
            let expected = self.fb.make_imm_value(index != 0);
            return Ok(self
                .fb
                .insert_inst(Eq::new(self.module.inst_set(), value, expected), Type::I1));
        }
        let variant = self.variant_ref(target, index)?;
        Ok(self.fb.insert_inst(
            EnumIsVariant::new(self.module.inst_set(), value, variant),
            Type::I1,
        ))
    }

    fn extract_variant(
        &mut self,
        value: ValueId,
        target: &HullTy<'db>,
        index: u32,
    ) -> Result<(HullTy<'db>, ValueId), TranslationError> {
        let (lhs, rhs) = sum_parts(target)?;
        let payload = if index == 0 { lhs } else { rhs };
        if is_bool_like(target) || is_unit_ty(payload) {
            return Ok((
                payload.clone(),
                self.fb.func.dfg.make_zero_const(Type::Unit),
            ));
        }
        let variant = self.variant_ref(target, index)?;
        self.fb.insert_inst_no_result(EnumAssertVariant::new(
            self.module.inst_set(),
            value,
            variant,
        ));
        let field = self.index_value(0);
        let payload_ty = self.module.lower_ty(payload)?;
        let value = self.fb.insert_inst(
            EnumExtract::new(self.module.inst_set(), value, variant, field),
            payload_ty,
        );
        Ok((payload.clone(), value))
    }

    fn in_k_condition(
        &mut self,
        value: ValueId,
        target: &HullTy<'db>,
        index: usize,
    ) -> Result<ValueId, TranslationError> {
        if !matches!(target.strip_named().kind, TyKind::Sum(_, _)) {
            return if index == 0 {
                Ok(self.fb.make_imm_value(true))
            } else {
                Err(TranslationError::new(format!(
                    "bad in({index}) pattern for non-sum Hull type"
                )))
            };
        }
        if index == 0 {
            return self.is_variant(value, target, 0);
        }
        if is_bool_like(target) {
            return if index == 1 {
                self.is_variant(value, target, 1)
            } else {
                Err(TranslationError::new(format!(
                    "bad in({index}) pattern for boolean sum"
                )))
            };
        }

        let result = self.fb.declare_var(Type::I1);
        let initial = self.fb.make_imm_value(false);
        self.def_var(result, initial);
        let right = self.fb.append_block();
        let not_right = self.fb.append_block();
        let done = self.fb.append_block();
        let is_right = self.is_variant(value, target, 1)?;
        self.emit_branch(is_right, right, not_right);
        self.fb.switch_to_block(right);
        let (rhs, nested) = self.extract_variant(value, target, 1)?;
        let nested_cond = self.in_k_condition(nested, &rhs, index - 1)?;
        self.def_var(result, nested_cond);
        self.emit_jump(done);
        self.fb.switch_to_block(not_right);
        let false_value = self.fb.make_imm_value(false);
        self.def_var(result, false_value);
        self.emit_jump(done);
        self.fb.switch_to_block(done);
        Ok(self.use_var(result))
    }

    fn extract_in_k(
        &mut self,
        value: ValueId,
        target: &HullTy<'db>,
        index: usize,
    ) -> Result<(HullTy<'db>, ValueId), TranslationError> {
        if !matches!(target.strip_named().kind, TyKind::Sum(_, _)) {
            return if index == 0 {
                Ok((target.clone(), value))
            } else {
                Err(TranslationError::new(format!(
                    "bad in({index}) payload for non-sum Hull type"
                )))
            };
        }
        if index == 0 {
            return self.extract_variant(value, target, 0);
        }
        let (rhs, nested) = self.extract_variant(value, target, 1)?;
        self.extract_in_k(nested, &rhs, index - 1)
    }

    fn lower_injection(
        &mut self,
        target: &HullTy<'db>,
        index: usize,
        payload: ValueId,
    ) -> Result<ValueId, TranslationError> {
        if !matches!(target.strip_named().kind, TyKind::Sum(_, _)) {
            if index != 0 {
                return Err(TranslationError::new(format!(
                    "bad in({index}) injection for non-sum Hull type"
                )));
            }
            let target_ty = self.module.lower_ty(target)?;
            return self.coerce(payload, target_ty);
        }
        if is_bool_like(target) {
            return match index {
                0 => Ok(self.fb.make_imm_value(false)),
                1 => Ok(self.fb.make_imm_value(true)),
                _ => Err(TranslationError::new(format!(
                    "bad in({index}) injection for boolean sum"
                ))),
            };
        }
        let (_, rhs) = sum_parts(target)?;
        if index == 0 {
            self.lower_variant(target, 0, payload)
        } else if index == 1 {
            let nested = self.lower_injection(rhs, 0, payload)?;
            self.lower_variant(target, 1, nested)
        } else {
            let nested = self.lower_injection(rhs, index - 1, payload)?;
            self.lower_variant(target, 1, nested)
        }
    }

    fn lower_variant(
        &mut self,
        target: &HullTy<'db>,
        variant_index: u32,
        payload: ValueId,
    ) -> Result<ValueId, TranslationError> {
        if is_bool_like(target) {
            return Ok(self.fb.make_imm_value(variant_index != 0));
        }
        let (lhs, rhs) = sum_parts(target)?;
        let payload_ty = if variant_index == 0 { lhs } else { rhs };
        let ty = self.module.lower_ty(target)?;
        let variant = self.variant_ref(target, variant_index)?;
        let values = if is_unit_ty(payload_ty) {
            SmallVec::new()
        } else {
            let target = self.module.lower_ty(payload_ty)?;
            smallvec![self.coerce(payload, target)?]
        };
        Ok(self.fb.insert_inst(
            EnumMake::new(self.module.inst_set(), ty, variant, values),
            ty,
        ))
    }

    fn variant_ref(
        &mut self,
        target: &HullTy<'db>,
        index: u32,
    ) -> Result<EnumVariantRef, TranslationError> {
        let ty = self.module.lower_ty(target)?;
        let Type::Compound(enum_ty) = ty else {
            return Err(TranslationError::new(
                "sum did not lower to a Sonatina enum",
            ));
        };
        if !matches!(
            ty.resolve_compound(&self.fb.module_builder.ctx),
            Some(CompoundType::Enum(_))
        ) {
            return Err(TranslationError::new("sum target is not a Sonatina enum"));
        }
        Ok(EnumVariantRef::new(enum_ty, index))
    }

    fn lower_condition(&mut self, expr: &HullExpr<'db>) -> Result<ValueId, TranslationError> {
        let value = self.lower_expr(expr)?;
        self.condition_value(value)
    }

    fn condition_value(&mut self, value: ValueId) -> Result<ValueId, TranslationError> {
        match self.fb.type_of(value) {
            Type::I1 => Ok(value),
            ty if ty.is_integral() => {
                let zero = zero_for_type(&mut self.fb, self.module.inst_set(), ty);
                let is_zero = self
                    .fb
                    .insert_inst(Eq::new(self.module.inst_set(), value, zero), Type::I1);
                Ok(self
                    .fb
                    .insert_inst(IsZero::new(self.module.inst_set(), is_zero), Type::I1))
            }
            ty => Err(TranslationError::new(format!(
                "cannot use Sonatina type `{ty:?}` as a condition"
            ))),
        }
    }

    fn coerce(&mut self, value: ValueId, target: Type) -> Result<ValueId, TranslationError> {
        let source = self.fb.type_of(value);
        if source == target {
            return Ok(value);
        }
        if source.is_integral() && target.is_integral() {
            if source < target {
                return Ok(self
                    .fb
                    .insert_inst(Zext::new(self.module.inst_set(), value, target), target));
            }
            return Ok(self
                .fb
                .insert_inst(Trunc::new(self.module.inst_set(), value, target), target));
        }
        Err(TranslationError::new(format!(
            "cannot coerce Sonatina value from `{source:?}` to `{target:?}`"
        )))
    }

    fn index_value(&mut self, index: usize) -> ValueId {
        self.fb.make_imm_value(I256::from(index))
    }

    fn word_value(&mut self, value: &str) -> Result<ValueId, TranslationError> {
        let wrapped =
            hull::wrap_word_literal(value).map_err(|err| TranslationError::new(err.to_string()))?;
        let immediate = if let Some(hex) = wrapped
            .strip_prefix("0x")
            .or_else(|| wrapped.strip_prefix("0X"))
        {
            I256::from_be_bytes(&decode_hex_word(hex)?)
        } else {
            let unsigned = sonatina_ir::U256::from_dec_str(&wrapped).map_err(|err| {
                TranslationError::new(format!("invalid 256-bit word literal `{value}`: {err}"))
            })?;
            I256::from_u256(unsigned)
        };
        Ok(self.fb.make_imm_value(immediate))
    }

    fn lower_builtin_call(
        &mut self,
        name: &str,
        args: &[ValueId],
        result: &HullTy<'db>,
    ) -> Result<ValueId, TranslationError> {
        if is_primitive_name(name) {
            return self.lower_primitive_call(name, args, result);
        }
        let result_ty = self.module.lower_ty(result)?;
        match self.lower_evm_builtin(name, args)? {
            BuiltinOutcome::Value(value) => self.coerce(value, result_ty),
            BuiltinOutcome::Unit => Ok(self.fb.func.dfg.make_zero_const(Type::Unit)),
            BuiltinOutcome::Terminated => Ok(self.fb.func.dfg.make_zero_const(Type::Unit)),
        }
    }

    fn lower_yul_stmts(&mut self, stmts: &[YulStmt<'db>]) -> Result<bool, TranslationError> {
        self.push_scope();
        self.declare_yul_functions(stmts)?;
        self.lower_yul_function_bodies(stmts)?;
        let terminated = self.lower_yul_stmt_seq(stmts)?;
        self.yul_functions.pop();
        self.pop_scope();
        Ok(terminated)
    }

    fn declare_yul_functions(&mut self, stmts: &[YulStmt<'db>]) -> Result<(), TranslationError> {
        let mut declarations = HashMap::new();
        for stmt in stmts {
            let YulStmtKind::FunctionDef {
                name, params, rets, ..
            } = &stmt.kind
            else {
                continue;
            };
            let source_name = self.yul_name(name);
            let declaration = match declarations.entry(source_name.clone()) {
                std::collections::hash_map::Entry::Vacant(declaration) => declaration,
                std::collections::hash_map::Entry::Occupied(_) => {
                    return Err(TranslationError::new(format!(
                        "duplicate inline Yul function `{source_name}` in the same block"
                    )));
                }
            };
            let index = self.module.next_yul_function;
            self.module.next_yul_function += 1;
            let symbol = yul_function_symbol(&self.scope, &source_name, index);
            let args = vec![Type::I256; params.len()];
            let returns = vec![Type::I256; rets.len()];
            let func = self
                .module
                .builder
                .declare_function(Signature::new(&symbol, Linkage::Private, &args, &returns))
                .map_err(|err| {
                    TranslationError::new(format!(
                        "failed to declare inline Yul function `{source_name}`: {err}"
                    ))
                })?;
            declaration.insert(YulFunctionDecl {
                func,
                params: params.len(),
                returns: rets.len(),
            });
        }
        self.yul_functions.push(declarations);
        Ok(())
    }

    fn lower_yul_function_bodies(
        &mut self,
        stmts: &[YulStmt<'db>],
    ) -> Result<(), TranslationError> {
        for stmt in stmts {
            let YulStmtKind::FunctionDef {
                name,
                params,
                rets,
                body,
            } = &stmt.kind
            else {
                continue;
            };
            let source_name = self.yul_name(name);
            let declaration = self
                .yul_functions
                .last()
                .and_then(|scope| scope.get(&source_name))
                .copied()
                .ok_or_else(|| {
                    TranslationError::new(format!(
                        "missing inline Yul declaration for `{source_name}`"
                    ))
                })?;
            self.lower_yul_function(stmt.span, declaration, params, rets, body)?;
        }
        Ok(())
    }

    fn lower_yul_function(
        &mut self,
        span: hir::span::Span<'db>,
        declaration: YulFunctionDecl,
        params: &[hir::span::SpannedElem<'db, hir::ast::Ident<'db>>],
        rets: &[hir::span::SpannedElem<'db, hir::ast::Ident<'db>>],
        body: &[YulStmt<'db>],
    ) -> Result<(), TranslationError> {
        let visible_functions = self.yul_functions.clone();
        let mut lowerer = FunctionLowerer::new_yul(
            self.module,
            &self.scope,
            declaration.func,
            span,
            visible_functions,
            self.function,
        );
        let word = HullTy::word(span);
        for (index, param) in params.iter().enumerate() {
            let name = lowerer.yul_name(param);
            let value = lowerer.fb.func.arg_values[index];
            lowerer.bind_parameter(&name, &word, value)?;
        }
        let mut return_vars = Vec::with_capacity(rets.len());
        for ret in rets {
            let name = lowerer.yul_name(ret);
            return_vars.push(lowerer.declare_binding(&name, &word)?.var);
        }
        lowerer.yul_returns = Some(return_vars);
        let terminated = lowerer.lower_yul_stmts(body)?;
        lowerer.finish_yul(terminated)
    }

    fn lookup_yul_function(&self, name: &str) -> Option<YulFunctionDecl> {
        self.yul_functions
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn lower_yul_stmt_seq(&mut self, stmts: &[YulStmt<'db>]) -> Result<bool, TranslationError> {
        for stmt in stmts {
            if self.lower_yul_stmt(stmt)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn lower_yul_stmt(&mut self, stmt: &YulStmt<'db>) -> Result<bool, TranslationError> {
        let start = self.visible;
        let watermark = self.fb.func.dfg.num_insts();
        let result = self.lower_yul_stmt_inner(stmt);
        self.stamp_insts_since(watermark, stmt.span, start, false);
        result
    }

    fn lower_yul_stmt_inner(&mut self, stmt: &YulStmt<'db>) -> Result<bool, TranslationError> {
        match &stmt.kind {
            YulStmtKind::Block(stmts) => self.lower_yul_stmts(stmts),
            YulStmtKind::Let { names, init } => {
                let values = match init {
                    Some(expr) => match self.lower_yul_expr_outcome(expr)? {
                        YulExprOutcome::Values(values) => values,
                        YulExprOutcome::Terminated => {
                            return Err(TranslationError::new(
                                "terminating inline Yul expression cannot initialize a binding",
                            ));
                        }
                    },
                    None => SmallVec::new(),
                };
                if init.is_some() && values.len() != names.len() {
                    return Err(TranslationError::new(format!(
                        "inline Yul let expects {} values, got {}",
                        names.len(),
                        values.len()
                    )));
                }
                for (index, name) in names.iter().enumerate() {
                    let name = self.yul_name(name);
                    let ty = HullTy::word(stmt.span);
                    let binding = self.declare_binding(&name, &ty)?;
                    if let Some(value) = values.get(index).copied() {
                        let value = self.coerce(value, Type::I256)?;
                        self.def_var(binding.var, value);
                    }
                }
                Ok(false)
            }
            YulStmtKind::Assign { names, value } => {
                let values = match self.lower_yul_expr_outcome(value)? {
                    YulExprOutcome::Values(values) => values,
                    YulExprOutcome::Terminated => {
                        return Err(TranslationError::new(
                            "terminating inline Yul expression cannot be assigned",
                        ));
                    }
                };
                if values.len() != names.len() {
                    return Err(TranslationError::new(format!(
                        "inline Yul assignment expects {} values, got {}",
                        names.len(),
                        values.len()
                    )));
                }
                for (name, value) in names.iter().zip(values) {
                    let name = self.yul_name(name);
                    let binding = self.lookup(&name)?;
                    let value = self.coerce(value, binding.ty)?;
                    self.def_var(binding.var, value);
                }
                Ok(false)
            }
            YulStmtKind::Expr(expr) => match self.lower_yul_expr_outcome(expr)? {
                YulExprOutcome::Terminated => Ok(true),
                YulExprOutcome::Values(values) if values.is_empty() => Ok(false),
                YulExprOutcome::Values(values) => Err(TranslationError::new(format!(
                    "inline Yul expression statement expects zero values, got {}",
                    values.len()
                ))),
            },
            YulStmtKind::If { cond, body } => {
                let body_block = self.fb.append_block();
                let done = self.fb.append_block();
                let cond = self.lower_yul_expr(cond)?;
                let cond = self.condition_value(cond)?;
                self.emit_branch(cond, body_block, done);
                self.fb.switch_to_block(body_block);
                let terminated = self.lower_yul_stmts(body)?;
                if !terminated {
                    self.emit_jump(done);
                }
                self.fb.switch_to_block(done);
                Ok(false)
            }
            YulStmtKind::For {
                init,
                cond,
                post,
                body,
            } => {
                self.push_scope();
                // Yul loop-init bindings are visible to the condition, post, and
                // body for the entire loop scope.
                self.declare_yul_functions(init)?;
                self.lower_yul_function_bodies(init)?;
                if self.lower_yul_stmt_seq(init)? {
                    self.yul_functions.pop();
                    self.pop_scope();
                    return Ok(true);
                }
                let header = self.fb.append_block();
                let body_block = self.fb.append_block();
                let post_block = self.fb.append_block();
                let done = self.fb.append_block();
                self.emit_jump(header);
                self.fb.switch_to_block(header);
                let cond = self.lower_yul_expr(cond)?;
                let cond = self.condition_value(cond)?;
                self.emit_branch(cond, body_block, done);
                self.fb.switch_to_block(body_block);
                self.break_targets.push(done);
                self.continue_targets.push(post_block);
                let body_terminated = self.lower_yul_stmts(body)?;
                self.continue_targets.pop();
                self.break_targets.pop();
                if !body_terminated {
                    self.emit_jump(post_block);
                }
                if self.switch_to_reachable_block(post_block) && !self.lower_yul_stmts(post)? {
                    self.emit_jump(header);
                }
                self.fb.switch_to_block(done);
                self.yul_functions.pop();
                self.pop_scope();
                Ok(false)
            }
            YulStmtKind::Switch {
                expr,
                cases,
                default,
            } => {
                let scrutinee = self.lower_yul_expr(expr)?;
                let done = self.fb.append_block();
                for case in cases {
                    let arm = self.fb.append_block();
                    let next = self.fb.append_block();
                    let expected = self.lower_yul_lit(&case.lit)?;
                    let cond = self.fb.insert_inst(
                        Eq::new(self.module.inst_set(), scrutinee, expected),
                        Type::I1,
                    );
                    self.emit_branch(cond, arm, next);
                    self.fb.switch_to_block(arm);
                    let terminated = self.lower_yul_stmts(&case.body)?;
                    if !terminated {
                        self.emit_jump(done);
                    }
                    self.fb.switch_to_block(next);
                }
                if let Some(default) = default {
                    let terminated = self.lower_yul_stmts(default)?;
                    if !terminated {
                        self.emit_jump(done);
                    }
                } else {
                    self.emit_jump(done);
                }
                Ok(!self.switch_to_reachable_block(done))
            }
            // Definitions are declared and lowered on entry to their lexical block.
            YulStmtKind::FunctionDef { .. } => Ok(false),
            YulStmtKind::Leave => {
                self.emit_yul_return()?;
                Ok(true)
            }
            YulStmtKind::Break => {
                let target = self
                    .break_targets
                    .last()
                    .copied()
                    .ok_or_else(|| TranslationError::new("inline Yul break outside loop"))?;
                self.emit_jump(target);
                Ok(true)
            }
            YulStmtKind::Continue => {
                let target = self
                    .continue_targets
                    .last()
                    .copied()
                    .ok_or_else(|| TranslationError::new("inline Yul continue outside loop"))?;
                self.emit_jump(target);
                Ok(true)
            }
            YulStmtKind::Error => Ok(false),
        }
    }

    fn lower_yul_expr(&mut self, expr: &YulExpr<'db>) -> Result<ValueId, TranslationError> {
        match self.lower_yul_expr_outcome(expr)? {
            YulExprOutcome::Values(values) if values.len() == 1 => Ok(values[0]),
            YulExprOutcome::Values(values) => Err(TranslationError::new(format!(
                "inline Yul expression requires one value, got {}",
                values.len()
            ))),
            YulExprOutcome::Terminated => Err(TranslationError::new(
                "terminating inline Yul expression cannot be used as a value",
            )),
        }
    }

    fn lower_yul_expr_outcome(
        &mut self,
        expr: &YulExpr<'db>,
    ) -> Result<YulExprOutcome, TranslationError> {
        match &expr.kind {
            YulExprKind::Lit(lit) => {
                Ok(YulExprOutcome::Values(smallvec![self.lower_yul_lit(lit)?]))
            }
            YulExprKind::Ident(name) => {
                let binding = self.lookup(&self.yul_name(name))?;
                let value = self.use_var(binding.var);
                Ok(YulExprOutcome::Values(smallvec![
                    self.coerce(value, Type::I256)?
                ]))
            }
            YulExprKind::Call { name, args } => {
                let name = self.yul_name(name);
                if matches!(name.as_str(), "dataoffset" | "datasize") {
                    let symbol = args
                        .first()
                        .and_then(yul_symbol)
                        .ok_or_else(|| TranslationError::new(format!("{name} expects a symbol")))?;
                    let sym = if self
                        .module
                        .section_objects
                        .get(&self.scope)
                        .is_some_and(|current| current == &symbol)
                    {
                        SymbolRef::CurrentSection
                    } else {
                        SymbolRef::Embed(EmbedSymbol::from(symbol))
                    };
                    let value = if name == "dataoffset" {
                        self.fb
                            .insert_inst(SymAddr::new(self.module.inst_set(), sym), Type::I256)
                    } else {
                        self.fb
                            .insert_inst(SymSize::new(self.module.inst_set(), sym), Type::I256)
                    };
                    return Ok(YulExprOutcome::Values(smallvec![value]));
                }
                // Yul evaluates call arguments from right to left. Restore source
                // position after lowering so the callee still receives arg N in
                // parameter N.
                let mut values = args
                    .iter()
                    .rev()
                    .map(|arg| self.lower_yul_expr(arg))
                    .collect::<Result<SmallVec<[ValueId; 8]>, _>>()?;
                values.reverse();
                if let Some(declaration) = self.lookup_yul_function(&name) {
                    if declaration.params != values.len() {
                        return Err(TranslationError::new(format!(
                            "inline Yul call to `{name}` expects {} arguments, got {}",
                            declaration.params,
                            values.len()
                        )));
                    }
                    let results = self.fb.insert_call_results(declaration.func, values);
                    if results.len() != declaration.returns {
                        return Err(TranslationError::new(format!(
                            "inline Yul call to `{name}` expected {} return values, got {}",
                            declaration.returns,
                            results.len()
                        )));
                    }
                    return Ok(YulExprOutcome::Values(results));
                }
                let source_name = name.strip_prefix("usr$").unwrap_or(&name);
                if let Some(func) = self
                    .module
                    .functions
                    .get(&key(&self.scope, source_name))
                    .copied()
                {
                    let param_tys = self
                        .fb
                        .module_builder
                        .sig(func, |signature| signature.args().to_vec());
                    if param_tys.len() != values.len() {
                        return Err(TranslationError::new(format!(
                            "inline Yul call to `{source_name}` expects {} arguments, got {}",
                            param_tys.len(),
                            values.len()
                        )));
                    }
                    let values = values
                        .into_iter()
                        .zip(param_tys)
                        .map(|(value, target)| {
                            if !target.is_integral() {
                                return Err(TranslationError::new(format!(
                                    "inline Yul cannot pass a word to aggregate parameter `{target:?}` of `{source_name}`"
                                )));
                            }
                            self.coerce(value, target)
                        })
                        .collect::<Result<SmallVec<[ValueId; 8]>, _>>()?;
                    let ret_hull = self
                        .module
                        .function_returns
                        .get(&key(&self.scope, source_name))
                        .cloned()
                        .ok_or_else(|| TranslationError::new("missing function return type"))?;
                    let ret = self.module.lower_ty(&ret_hull)?;
                    return if ret == Type::Unit {
                        let results = self.fb.insert_call_results(func, values);
                        Ok(YulExprOutcome::Values(results))
                    } else if ret.is_integral() {
                        let results = self.fb.insert_call_results(func, values);
                        let value = results.first().copied().ok_or_else(|| {
                            TranslationError::new(format!(
                                "inline Yul call to `{source_name}` produced no value"
                            ))
                        })?;
                        Ok(YulExprOutcome::Values(smallvec![
                            self.coerce(value, Type::I256)?
                        ]))
                    } else {
                        Err(TranslationError::new(format!(
                            "inline Yul cannot use aggregate return type `{ret:?}` from `{source_name}`"
                        )))
                    };
                }
                match self.lower_evm_builtin(&name, &values)? {
                    BuiltinOutcome::Value(value) => Ok(YulExprOutcome::Values(smallvec![
                        self.coerce(value, Type::I256)?
                    ])),
                    BuiltinOutcome::Unit => Ok(YulExprOutcome::Values(SmallVec::new())),
                    BuiltinOutcome::Terminated => Ok(YulExprOutcome::Terminated),
                }
            }
            YulExprKind::Error => Ok(YulExprOutcome::Values(smallvec![
                self.fb.make_imm_value(I256::zero())
            ])),
        }
    }

    fn lower_yul_lit(&mut self, lit: &YulLitKind) -> Result<ValueId, TranslationError> {
        match lit {
            YulLitKind::Number(value) | YulLitKind::Hex(value) => self.word_value(value),
            YulLitKind::Bool(value) => Ok(self.fb.make_imm_value(I256::from(u8::from(*value)))),
            YulLitKind::String(value) => {
                let value = hir::string_literal::decode_string_literal(value)
                    .ok_or_else(|| TranslationError::new("invalid inline Yul string literal"))?;
                if value.len() > 32 {
                    return Err(TranslationError::new(
                        "inline Yul string literal exceeds one EVM word",
                    ));
                }
                let mut bytes = [0u8; 32];
                bytes[..value.len()].copy_from_slice(value.as_bytes());
                Ok(self.fb.make_imm_value(I256::from_be_bytes(&bytes)))
            }
            YulLitKind::Error => Ok(self.fb.make_imm_value(I256::zero())),
        }
    }

    fn yul_name(&self, name: &hir::span::SpannedElem<'db, hir::ast::Ident<'db>>) -> String {
        (*name.atom()).text(self.module.db).to_owned()
    }

    fn lower_evm_builtin(
        &mut self,
        name: &str,
        args: &[ValueId],
    ) -> Result<BuiltinOutcome, TranslationError> {
        let arg = |index: usize| {
            args.get(index).copied().ok_or_else(|| {
                TranslationError::new(format!("EVM builtin `{name}` is missing argument {index}"))
            })
        };
        let word = |value| BuiltinOutcome::Value(value);
        let outcome = match name {
            "memoryguard" => {
                let size = arg(0)?;
                let align = self.fb.make_imm_value(I256::from(31u8));
                let size = self
                    .fb
                    .insert_inst(Add::new(self.module.inst_set(), size, align), Type::I256);
                let align_mask = self.fb.make_imm_value(!I256::from(31u8));
                let size = self.fb.insert_inst(
                    And::new(self.module.inst_set(), size, align_mask),
                    Type::I256,
                );
                let ptr_ty = Type::I8.to_ptr(self.fb.ctx());
                // Solcore's generated dispatcher stores `memoryguard(literal)` in
                // the free-pointer slot. Reserve that region through Sonatina's
                // unified allocator and return its aligned end, rather than
                // rewinding the free pointer into the backend's frame arena.
                let base = self
                    .fb
                    .insert_inst(EvmMalloc::new(self.module.inst_set(), size), ptr_ty);
                let base = self.fb.insert_inst(
                    PtrToInt::new(self.module.inst_set(), base, Type::I256),
                    Type::I256,
                );
                word(
                    self.fb
                        .insert_inst(Add::new(self.module.inst_set(), base, size), Type::I256),
                )
            }
            "add" => word(self.fb.insert_inst(
                Add::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "sub" => word(self.fb.insert_inst(
                Sub::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "mul" => word(self.fb.insert_inst(
                Mul::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "div" => word(self.fb.insert_inst(
                EvmUdiv::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "sdiv" => word(self.fb.insert_inst(
                EvmSdiv::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "mod" => word(self.fb.insert_inst(
                EvmUmod::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "smod" => word(self.fb.insert_inst(
                EvmSmod::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "addmod" => word(self.fb.insert_inst(
                EvmAddMod::new(self.module.inst_set(), arg(0)?, arg(1)?, arg(2)?),
                Type::I256,
            )),
            "mulmod" => word(self.fb.insert_inst(
                EvmMulMod::new(self.module.inst_set(), arg(0)?, arg(1)?, arg(2)?),
                Type::I256,
            )),
            "exp" => word(self.fb.insert_inst(
                EvmExp::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "signextend" => word(self.fb.insert_inst(
                EvmSignExtend::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "lt" => word(
                self.fb
                    .insert_inst(Lt::new(self.module.inst_set(), arg(0)?, arg(1)?), Type::I1),
            ),
            "gt" => word(
                self.fb
                    .insert_inst(Gt::new(self.module.inst_set(), arg(0)?, arg(1)?), Type::I1),
            ),
            "slt" => word(
                self.fb
                    .insert_inst(Slt::new(self.module.inst_set(), arg(0)?, arg(1)?), Type::I1),
            ),
            "sgt" => word(
                self.fb
                    .insert_inst(Sgt::new(self.module.inst_set(), arg(0)?, arg(1)?), Type::I1),
            ),
            "eq" => word(
                self.fb
                    .insert_inst(Eq::new(self.module.inst_set(), arg(0)?, arg(1)?), Type::I1),
            ),
            "iszero" => word(
                self.fb
                    .insert_inst(IsZero::new(self.module.inst_set(), arg(0)?), Type::I1),
            ),
            "and" => word(self.fb.insert_inst(
                And::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "or" => word(self.fb.insert_inst(
                Or::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "xor" => word(self.fb.insert_inst(
                Xor::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "not" => word(
                self.fb
                    .insert_inst(Not::new(self.module.inst_set(), arg(0)?), Type::I256),
            ),
            "byte" => word(self.fb.insert_inst(
                EvmByte::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "shl" => word(self.fb.insert_inst(
                Shl::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "shr" => word(self.fb.insert_inst(
                Shr::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "sar" => word(self.fb.insert_inst(
                Sar::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "clz" => word(
                self.fb
                    .insert_inst(Clz::new(self.module.inst_set(), arg(0)?), Type::I256),
            ),
            "keccak256" => word(self.fb.insert_inst(
                EvmKeccak256::new(self.module.inst_set(), arg(0)?, arg(1)?),
                Type::I256,
            )),
            "address" => word(
                self.fb
                    .insert_inst(EvmAddress::new(self.module.inst_set()), Type::I256),
            ),
            "balance" => word(
                self.fb
                    .insert_inst(EvmBalance::new(self.module.inst_set(), arg(0)?), Type::I256),
            ),
            "origin" => word(
                self.fb
                    .insert_inst(EvmOrigin::new(self.module.inst_set()), Type::I256),
            ),
            "caller" => word(
                self.fb
                    .insert_inst(EvmCaller::new(self.module.inst_set()), Type::I256),
            ),
            "callvalue" => word(
                self.fb
                    .insert_inst(EvmCallValue::new(self.module.inst_set()), Type::I256),
            ),
            "calldataload" => word(self.fb.insert_inst(
                EvmCalldataLoad::new(self.module.inst_set(), arg(0)?),
                Type::I256,
            )),
            "calldatasize" => word(
                self.fb
                    .insert_inst(EvmCalldataSize::new(self.module.inst_set()), Type::I256),
            ),
            "calldatacopy" => {
                self.fb.insert_inst_no_result(EvmCalldataCopy::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                ));
                BuiltinOutcome::Unit
            }
            "codesize" => word(
                self.fb
                    .insert_inst(EvmCodeSize::new(self.module.inst_set()), Type::I256),
            ),
            "codecopy" | "datacopy" => {
                self.fb.insert_inst_no_result(EvmCodeCopy::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                ));
                BuiltinOutcome::Unit
            }
            "gasprice" => word(
                self.fb
                    .insert_inst(EvmGasPrice::new(self.module.inst_set()), Type::I256),
            ),
            "extcodesize" => word(self.fb.insert_inst(
                EvmExtCodeSize::new(self.module.inst_set(), arg(0)?),
                Type::I256,
            )),
            "extcodecopy" => {
                self.fb.insert_inst_no_result(EvmExtCodeCopy::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                    arg(3)?,
                ));
                BuiltinOutcome::Unit
            }
            "returndatasize" => word(
                self.fb
                    .insert_inst(EvmReturnDataSize::new(self.module.inst_set()), Type::I256),
            ),
            "returndatacopy" => {
                self.fb.insert_inst_no_result(EvmReturnDataCopy::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                ));
                BuiltinOutcome::Unit
            }
            "extcodehash" => word(self.fb.insert_inst(
                EvmExtCodeHash::new(self.module.inst_set(), arg(0)?),
                Type::I256,
            )),
            "blockhash" => word(self.fb.insert_inst(
                EvmBlockHash::new(self.module.inst_set(), arg(0)?),
                Type::I256,
            )),
            "coinbase" => word(
                self.fb
                    .insert_inst(EvmCoinBase::new(self.module.inst_set()), Type::I256),
            ),
            "timestamp" => word(
                self.fb
                    .insert_inst(EvmTimestamp::new(self.module.inst_set()), Type::I256),
            ),
            "number" => word(
                self.fb
                    .insert_inst(EvmNumber::new(self.module.inst_set()), Type::I256),
            ),
            "prevrandao" | "difficulty" => word(
                self.fb
                    .insert_inst(EvmPrevRandao::new(self.module.inst_set()), Type::I256),
            ),
            "gaslimit" => word(
                self.fb
                    .insert_inst(EvmGasLimit::new(self.module.inst_set()), Type::I256),
            ),
            "chainid" => word(
                self.fb
                    .insert_inst(EvmChainId::new(self.module.inst_set()), Type::I256),
            ),
            "selfbalance" => word(
                self.fb
                    .insert_inst(EvmSelfBalance::new(self.module.inst_set()), Type::I256),
            ),
            "basefee" => word(
                self.fb
                    .insert_inst(EvmBaseFee::new(self.module.inst_set()), Type::I256),
            ),
            "blobhash" => word(self.fb.insert_inst(
                EvmBlobHash::new(self.module.inst_set(), arg(0)?),
                Type::I256,
            )),
            "blobbasefee" => word(
                self.fb
                    .insert_inst(EvmBlobBaseFee::new(self.module.inst_set()), Type::I256),
            ),
            "mload" => word(
                self.fb
                    .insert_inst(EvmMload::new(self.module.inst_set(), arg(0)?), Type::I256),
            ),
            "mstore" => {
                self.fb.insert_inst_no_result(EvmMstore::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                ));
                BuiltinOutcome::Unit
            }
            "mstore8" => {
                let value = self.fb.insert_inst(
                    Trunc::new(self.module.inst_set(), arg(1)?, Type::I8),
                    Type::I8,
                );
                self.fb.insert_inst_no_result(EvmMstore8::new(
                    self.module.inst_set(),
                    arg(0)?,
                    value,
                ));
                BuiltinOutcome::Unit
            }
            "sload" => word(
                self.fb
                    .insert_inst(EvmSload::new(self.module.inst_set(), arg(0)?), Type::I256),
            ),
            "sstore" => {
                self.fb.insert_inst_no_result(EvmSstore::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                ));
                BuiltinOutcome::Unit
            }
            "tload" => word(
                self.fb
                    .insert_inst(EvmTload::new(self.module.inst_set(), arg(0)?), Type::I256),
            ),
            "tstore" => {
                self.fb.insert_inst_no_result(EvmTstore::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                ));
                BuiltinOutcome::Unit
            }
            "msize" => word(
                self.fb
                    .insert_inst(EvmMsize::new(self.module.inst_set()), Type::I256),
            ),
            "gas" => word(
                self.fb
                    .insert_inst(EvmGas::new(self.module.inst_set()), Type::I256),
            ),
            "mcopy" => {
                self.fb.insert_inst_no_result(EvmMcopy::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                ));
                BuiltinOutcome::Unit
            }
            "log0" => {
                self.fb.insert_inst_no_result(EvmLog0::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                ));
                BuiltinOutcome::Unit
            }
            "log1" => {
                self.fb.insert_inst_no_result(EvmLog1::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                ));
                BuiltinOutcome::Unit
            }
            "log2" => {
                self.fb.insert_inst_no_result(EvmLog2::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                    arg(3)?,
                ));
                BuiltinOutcome::Unit
            }
            "log3" => {
                self.fb.insert_inst_no_result(EvmLog3::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                    arg(3)?,
                    arg(4)?,
                ));
                BuiltinOutcome::Unit
            }
            "log4" => {
                self.fb.insert_inst_no_result(EvmLog4::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                    arg(3)?,
                    arg(4)?,
                    arg(5)?,
                ));
                BuiltinOutcome::Unit
            }
            "create" => word(self.fb.insert_inst(
                EvmCreate::new(self.module.inst_set(), arg(0)?, arg(1)?, arg(2)?),
                Type::I256,
            )),
            "create2" => word(self.fb.insert_inst(
                EvmCreate2::new(self.module.inst_set(), arg(0)?, arg(1)?, arg(2)?, arg(3)?),
                Type::I256,
            )),
            "call" => word(self.fb.insert_inst(
                EvmCall::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                    arg(3)?,
                    arg(4)?,
                    arg(5)?,
                    arg(6)?,
                ),
                Type::I256,
            )),
            "callcode" => word(self.fb.insert_inst(
                EvmCallCode::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                    arg(3)?,
                    arg(4)?,
                    arg(5)?,
                    arg(6)?,
                ),
                Type::I256,
            )),
            "delegatecall" => word(self.fb.insert_inst(
                EvmDelegateCall::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                    arg(3)?,
                    arg(4)?,
                    arg(5)?,
                ),
                Type::I256,
            )),
            "staticcall" => word(self.fb.insert_inst(
                EvmStaticCall::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                    arg(2)?,
                    arg(3)?,
                    arg(4)?,
                    arg(5)?,
                ),
                Type::I256,
            )),
            "return" => {
                self.fb.insert_inst_no_result(EvmReturn::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                ));
                BuiltinOutcome::Terminated
            }
            "revert" => {
                self.fb.insert_inst_no_result(EvmRevert::new(
                    self.module.inst_set(),
                    arg(0)?,
                    arg(1)?,
                ));
                BuiltinOutcome::Terminated
            }
            "stop" => {
                self.fb
                    .insert_inst_no_result(EvmStop::new(self.module.inst_set()));
                BuiltinOutcome::Terminated
            }
            "invalid" => {
                self.fb
                    .insert_inst_no_result(EvmInvalid::new(self.module.inst_set()));
                BuiltinOutcome::Terminated
            }
            "selfdestruct" => {
                self.fb
                    .insert_inst_no_result(EvmSelfDestruct::new(self.module.inst_set(), arg(0)?));
                BuiltinOutcome::Terminated
            }
            "pop" => BuiltinOutcome::Unit,
            _ => {
                return Err(TranslationError::new(format!(
                    "unsupported inline Yul/EVM builtin `{name}`"
                )));
            }
        };
        Ok(outcome)
    }

    fn lower_primitive_call(
        &mut self,
        name: &str,
        args: &[ValueId],
        result: &HullTy<'db>,
    ) -> Result<ValueId, TranslationError> {
        let result_ty = self.module.lower_ty(result)?;
        let binary = |args: &[ValueId]| -> Result<(ValueId, ValueId), TranslationError> {
            let [lhs, rhs] = args else {
                return Err(TranslationError::new(format!(
                    "builtin `{name}` expects two arguments"
                )));
            };
            Ok((*lhs, *rhs))
        };
        let value = match name {
            "wordFromInteger" | "wordToInteger" => *args.first().ok_or_else(|| {
                TranslationError::new(format!("builtin `{name}` expects one argument"))
            })?,
            "add" | "primAddWord" | "integerAdd" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Add::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "sub" | "subWord" | "integerSub" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Sub::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "mul" | "mulWord" | "integerMul" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Mul::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "div" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(EvmUdiv::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "sdiv" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(EvmSdiv::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "mod" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(EvmUmod::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "smod" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(EvmSmod::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "eq" | "primEqWord" | "integerEq" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Eq::new(self.module.inst_set(), lhs, rhs), Type::I1)
            }
            "lt" | "integerLt" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Lt::new(self.module.inst_set(), lhs, rhs), Type::I1)
            }
            "gt" | "gtWord" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Gt::new(self.module.inst_set(), lhs, rhs), Type::I1)
            }
            "slt" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Slt::new(self.module.inst_set(), lhs, rhs), Type::I1)
            }
            "and" | "bandWord" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(And::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "or" | "borWord" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Or::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "xor" | "bxorWord" => {
                let (lhs, rhs) = binary(args)?;
                self.fb
                    .insert_inst(Xor::new(self.module.inst_set(), lhs, rhs), Type::I256)
            }
            "not" | "bnotWord" => {
                let value = *args
                    .first()
                    .ok_or_else(|| TranslationError::new("not expects one argument"))?;
                self.fb
                    .insert_inst(Not::new(self.module.inst_set(), value), Type::I256)
            }
            "iszero" => {
                let value = *args
                    .first()
                    .ok_or_else(|| TranslationError::new("iszero expects one argument"))?;
                self.fb
                    .insert_inst(IsZero::new(self.module.inst_set(), value), Type::I1)
            }
            "shl" => {
                let (bits, value) = binary(args)?;
                self.fb
                    .insert_inst(Shl::new(self.module.inst_set(), bits, value), Type::I256)
            }
            "shr" => {
                let (bits, value) = binary(args)?;
                self.fb
                    .insert_inst(Shr::new(self.module.inst_set(), bits, value), Type::I256)
            }
            "sar" => {
                let (bits, value) = binary(args)?;
                self.fb
                    .insert_inst(Sar::new(self.module.inst_set(), bits, value), Type::I256)
            }
            "exp" => {
                let (base, exponent) = binary(args)?;
                self.fb.insert_inst(
                    EvmExp::new(self.module.inst_set(), base, exponent),
                    Type::I256,
                )
            }
            _ => {
                return Err(TranslationError::new(format!(
                    "unsupported Hull/EVM builtin `{name}`"
                )));
            }
        };
        self.coerce(value, result_ty)
    }
}

fn zero_for_type(
    fb: &mut FunctionBuilder<InstInserter>,
    inst_set: &'static EvmInstSet,
    ty: Type,
) -> ValueId {
    if ty == Type::Unit {
        return fb.func.dfg.make_zero_const(ty);
    }
    let Type::Compound(compound_ref) = ty else {
        return fb.make_imm_value(Immediate::zero(ty));
    };
    let Some(compound) = ty.resolve_compound(fb.ctx()) else {
        return fb.func.dfg.make_zero_const(ty);
    };
    match compound {
        CompoundType::Struct(data) => {
            let mut value = fb.func.dfg.make_zero_const(ty);
            for (index, field_ty) in data.fields.into_iter().enumerate() {
                let field = zero_for_type(fb, inst_set, field_ty);
                let index = fb.make_imm_value(I256::from(index));
                value = fb.insert_inst(InsertValue::new(inst_set, value, index, field), ty);
            }
            value
        }
        CompoundType::Array { elem, len } => {
            let mut value = fb.func.dfg.make_zero_const(ty);
            for index in 0..len {
                let field = zero_for_type(fb, inst_set, elem);
                let index = fb.make_imm_value(I256::from(index));
                value = fb.insert_inst(InsertValue::new(inst_set, value, index, field), ty);
            }
            value
        }
        CompoundType::Enum(data) => {
            let Some(variant) = data.variants.first() else {
                return fb.func.dfg.make_zero_const(ty);
            };
            let fields = variant
                .fields
                .iter()
                .copied()
                .map(|field_ty| zero_for_type(fb, inst_set, field_ty))
                .collect::<SmallVec<[ValueId; 2]>>();
            fb.insert_inst(
                EnumMake::new(inst_set, ty, EnumVariantRef::new(compound_ref, 0), fields),
                ty,
            )
        }
        CompoundType::Ptr(_)
        | CompoundType::ObjRef(_)
        | CompoundType::ConstRef(_)
        | CompoundType::Func { .. } => fb.func.dfg.make_zero_const(ty),
    }
}

fn is_primitive_name(name: &str) -> bool {
    matches!(
        name,
        "wordFromInteger"
            | "wordToInteger"
            | "add"
            | "primAddWord"
            | "integerAdd"
            | "sub"
            | "subWord"
            | "integerSub"
            | "mul"
            | "mulWord"
            | "integerMul"
            | "div"
            | "sdiv"
            | "mod"
            | "smod"
            | "eq"
            | "primEqWord"
            | "integerEq"
            | "lt"
            | "integerLt"
            | "gt"
            | "gtWord"
            | "slt"
            | "and"
            | "bandWord"
            | "or"
            | "borWord"
            | "xor"
            | "bxorWord"
            | "not"
            | "bnotWord"
            | "iszero"
            | "shl"
            | "shr"
            | "sar"
            | "exp"
    )
}

fn is_terminal_evm_builtin(name: &str) -> bool {
    matches!(
        name,
        "return" | "revert" | "stop" | "invalid" | "selfdestruct"
    )
}

fn yul_symbol(expr: &YulExpr<'_>) -> Option<String> {
    match &expr.kind {
        YulExprKind::Lit(YulLitKind::String(value)) => {
            hir::string_literal::decode_string_literal(value)
        }
        _ => None,
    }
}

fn decode_hex_word(hex: &str) -> Result<Vec<u8>, TranslationError> {
    if hex.len() > 64 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(TranslationError::new(format!(
            "invalid 256-bit hexadecimal literal `0x{hex}`"
        )));
    }
    let padded = if hex.len().is_multiple_of(2) {
        hex.to_owned()
    } else {
        format!("0{hex}")
    };
    (0..padded.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&padded[index..index + 2], 16)
                .map_err(|err| TranslationError::new(err.to_string()))
        })
        .collect()
}

fn assignment_path<'a>(expr: &'a HullExpr<'_>) -> Result<(&'a str, Vec<usize>), TranslationError> {
    fn go<'a>(expr: &'a HullExpr<'_>, out: &mut Vec<usize>) -> Option<&'a str> {
        match &expr.kind {
            ExprKind::Var(name) => Some(name.as_str()),
            ExprKind::Fst(inner) => {
                let root = go(inner, out)?;
                out.push(0);
                Some(root)
            }
            ExprKind::Snd(inner) => {
                let root = go(inner, out)?;
                out.push(1);
                Some(root)
            }
            _ => None,
        }
    }
    let mut path = Vec::new();
    let root = go(expr, &mut path)
        .ok_or_else(|| TranslationError::new("unsupported Hull assignment target"))?;
    Ok((root, path))
}

fn product_parts<'a, 'db>(
    ty: &'a HullTy<'db>,
) -> Result<(&'a HullTy<'db>, &'a HullTy<'db>), TranslationError> {
    match &ty.strip_named().kind {
        TyKind::Product(lhs, rhs) => Ok((lhs, rhs)),
        _ => Err(TranslationError::new("expected Hull product type")),
    }
}

fn sum_parts<'a, 'db>(
    ty: &'a HullTy<'db>,
) -> Result<(&'a HullTy<'db>, &'a HullTy<'db>), TranslationError> {
    match &ty.strip_named().kind {
        TyKind::Sum(lhs, rhs) => Ok((lhs, rhs)),
        _ => Err(TranslationError::new("expected Hull sum type")),
    }
}

fn is_bool_like(ty: &HullTy<'_>) -> bool {
    matches!(ty.strip_named().kind, TyKind::Bool)
        || matches!(
            &ty.strip_named().kind,
            TyKind::Sum(lhs, rhs) if is_unit_ty(lhs) && is_unit_ty(rhs)
        )
}

fn is_unit_ty(ty: &HullTy<'_>) -> bool {
    matches!(ty.strip_named().kind, TyKind::Unit)
}

fn key(scope: &str, name: &str) -> String {
    format!("{scope}::{name}")
}

fn function_symbol(scope: &str, name: &str) -> String {
    format!(
        "solcore_fn_{}_{}",
        encode_symbol_component(scope),
        encode_symbol_component(name)
    )
}

fn yul_function_symbol(scope: &str, name: &str, index: usize) -> String {
    format!(
        "solcore_yul_fn_{}_{}_{}",
        encode_symbol_component(scope),
        index,
        encode_symbol_component(name)
    )
}

fn entry_symbol(scope: &str) -> String {
    format!("solcore_entry_{}", encode_symbol_component(scope))
}

fn object_section_name(scope: &str) -> String {
    format!("solcore_object_{}", encode_symbol_component(scope))
}

fn encode_symbol_component(source: &str) -> String {
    use std::fmt::Write as _;

    let mut out = format!("{}_", source.len());
    for byte in source.bytes() {
        if byte.is_ascii_alphanumeric() {
            out.push(char::from(byte));
        } else {
            write!(&mut out, "_{byte:02x}").expect("writing to String cannot fail");
        }
    }
    out
}

fn sanitize(source: &str) -> String {
    let mut out = String::new();
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "anon".to_owned()
    } else {
        out
    }
}

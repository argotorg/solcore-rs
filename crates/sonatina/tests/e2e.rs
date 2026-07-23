use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use dir_test::{Fixture, dir_test};
use hir::ast::item::{ContractItem, FuncKind, FunctionDef, Item, Module};
use hir_ty::{AbiParam, AbiType};
use hull::Program;
use nameres::{Db as _, module_id_from_key};
use parser::parse_file_to_hir;
use solcore_sonatina::translate_hull_program;
use solcore_test_utils::{
    define_frontend_test_db,
    e2e::{
        AbiShape, E2eFailure, FailureKind, ResolvedE2eCall, e2e_enabled, e2e_pipeline_only,
        e2e_required, encode_hex, parse_e2e_directive, resolve_e2e_comments,
        with_shared_evm_harness,
    },
    load_fixture_case_with_file_urls, load_reachable_modules_with_file_urls,
    repo_root_from_manifest,
};
use sonatina_codegen::{EvmCompile, OptLevel};
use specialize::{
    MonoEntry, MonoItem, MonoModule, MonoRuntimeMainOrigin, SpecializeOptions, specialize_module,
};

define_frontend_test_db!(TestDb, hir_ty);

type CompiledFixture = (Vec<(OptLevel, Vec<u8>)>, Vec<ResolvedE2eCall>);

#[dir_test(
    dir: "$CARGO_MANIFEST_DIR/../../tests/e2e",
    glob: "**/main.solc"
)]
fn sonatina_evm_e2e(fixture: Fixture<&str>) {
    if !e2e_enabled() {
        assert!(
            !e2e_required(),
            "E2E_REQUIRED=1 requires E2E=1; refusing to skip Sonatina E2E"
        );
        return;
    }

    let path = PathBuf::from(fixture.path());
    let result = lower_and_compile(&path).and_then(|(creations, calls)| {
        if e2e_pipeline_only() {
            return Ok(());
        }
        with_shared_evm_harness(|harness| {
            let Some(harness) = harness else {
                return Ok(());
            };
            for (opt_level, creation) in creations {
                harness
                    .execute_deployed_calls(&encode_hex(&creation), &calls)
                    .map_err(|failure| {
                        E2eFailure::new(
                            failure.kind,
                            format!("{opt_level:?} execution failed: {}", failure.message),
                        )
                    })?;
            }
            Ok(())
        })
    });

    result.unwrap_or_else(|failure| {
        panic!(
            "Sonatina E2E fixture `{}` failed: {failure}",
            path.display()
        )
    });
}

fn lower_and_compile(path: &Path) -> Result<CompiledFixture, E2eFailure> {
    let lowered = lower_fixture(path)?;
    let creations = [OptLevel::O0, OptLevel::O2]
        .into_iter()
        .map(|opt_level| {
            compile_creation(lowered.db, &lowered.program, opt_level)
                .map(|creation| (opt_level, creation))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((creations, lowered.calls))
}

fn compile_creation(
    db: &'static TestDb,
    program: &Program<'static>,
    opt_level: OptLevel,
) -> Result<Vec<u8>, E2eFailure> {
    let module = translate_hull_program(db, program).map_err(|error| {
        pipeline_error(format!(
            "Hull-to-Sonatina translation failed: {}",
            error.message()
        ))
    })?;
    let mut artifacts = EvmCompile::new(module)
        .with_opt_level(opt_level)
        .compile()
        .map_err(|errors| {
            pipeline_error(format!("Sonatina {opt_level:?} codegen failed: {errors:?}"))
        })?;
    if artifacts.len() != 1 {
        return Err(pipeline_error(format!(
            "expected one Sonatina object artifact, got {} ({})",
            artifacts.len(),
            artifacts
                .iter()
                .map(|artifact| artifact.object.0.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let artifact = artifacts.pop().expect("artifact count checked");
    artifact
        .sections
        .into_iter()
        .find_map(|(name, section)| (name.0.as_str() == "init").then_some(section.bytes))
        .ok_or_else(|| {
            pipeline_error(format!(
                "Sonatina object `{}` has no init section",
                artifact.object.0
            ))
        })
}

struct LoweredFixture {
    db: &'static TestDb,
    program: Program<'static>,
    calls: Vec<ResolvedE2eCall>,
}

fn lower_fixture(path: &Path) -> Result<LoweredFixture, E2eFailure> {
    let db = Box::leak(Box::new(TestDb::default()));
    let repo = repo_root();
    let case_dir = path
        .parent()
        .ok_or_else(|| pipeline_error(format!("fixture {} has no parent", path.display())))?;
    let entry = load_fixture_case_with_file_urls(db, case_dir, &repo, BTreeMap::new());
    load_reachable_modules_with_file_urls(db, entry.clone());
    let db: &'static TestDb = &*db;
    let entry_id = module_id_from_key(db, &entry);
    let file = db
        .module_file(entry_id)
        .ok_or_else(|| pipeline_error("entry source file is missing"))?;
    let hir = parse_file_to_hir(db, file).module(db);
    let specialized = specialize_module(db, hir, SpecializeOptions::default());
    finish_lowering(db, specialized)
}

fn finish_lowering(
    db: &'static TestDb,
    specialized: specialize::SpecializeOutput<'static>,
) -> Result<LoweredFixture, E2eFailure> {
    if !specialized.diagnostics.is_empty() {
        return Err(pipeline_error(format!(
            "specialization diagnostics: {:?}",
            specialized.diagnostics
        )));
    }
    let source = parse_file_to_hir(db, specialized.module.module.file(db)).module(db);
    let directives = resolve_fixture_directives(db, source, &specialized.module)?;
    let emitted = hull::emit_module(db, &specialized.module, hull::EmitOptions::default());
    if !emitted.diagnostics.is_empty() {
        return Err(pipeline_error(format!(
            "Hull emission diagnostics: {:?}",
            emitted.diagnostics
        )));
    }
    let diagnostics = hull::check_program_with_db(db, &emitted.program);
    if !diagnostics.is_empty() {
        return Err(pipeline_error(format!(
            "Hull check diagnostics: {diagnostics:?}"
        )));
    }

    let program = contract_program(&emitted.program, &directives.contract)?;
    Ok(LoweredFixture {
        db,
        program,
        calls: directives.calls,
    })
}

struct ResolvedFixtureDirectives {
    contract: String,
    calls: Vec<ResolvedE2eCall>,
}

fn resolve_fixture_directives(
    db: &'static TestDb,
    source: Module<'static>,
    specialized: &MonoModule<'static>,
) -> Result<ResolvedFixtureDirectives, E2eFailure> {
    let mut selected_contract = None::<String>;
    let mut calls = Vec::new();

    for item in source.items(db) {
        match item {
            Item::FunctionDef(function) => {
                reject_non_dispatch_directives(db, *function, "top-level function")?;
            }
            Item::InstanceDef(instance) => {
                for function in instance.methods(db) {
                    reject_non_dispatch_directives(db, *function, "impl method")?;
                }
            }
            Item::ContractDef(contract) => {
                let contract_name = contract.name_elem(db).atom().text(db).to_owned();
                let contract_def = contract.def_id_value(db);
                let surface = hir_ty::contract_dispatch_surface_for_module(db, source, *contract);
                let mono_contract = specialized
                    .items
                    .iter()
                    .find_map(|item| match item {
                        MonoItem::Contract(contract) if contract.def == contract_def => {
                            Some(contract)
                        }
                        _ => None,
                    })
                    .ok_or_else(|| {
                        directive_error(format!(
                            "{contract_name}: specialized contract metadata is missing"
                        ))
                    })?;
                let has_dispatch_runtime = mono_contract.entries.iter().any(|entry| {
                    matches!(
                        entry,
                        MonoEntry::RuntimeMain {
                            origin: MonoRuntimeMainOrigin::StdDispatch,
                            ..
                        }
                    )
                });

                for item in contract.items(db) {
                    let ContractItem::FunctionDef(function) = item else {
                        continue;
                    };
                    let comments = comment_texts(db, *function);
                    let function_name = function_name(db, *function);
                    let context = format!("{contract_name}::{function_name}");
                    if !contains_directive(&comments, &context)? {
                        continue;
                    }
                    if function.kind(db) != FuncKind::Function {
                        return Err(directive_error(format!(
                            "{context}: directives may only target ordinary public functions"
                        )));
                    }
                    if function.sig(db).public.is_none() {
                        return Err(directive_error(format!(
                            "{context}: directive target is private and has no external selector"
                        )));
                    }
                    if function_name == "main" {
                        return Err(directive_error(format!(
                            "{context}: contract runtime `main` is not a selector-dispatched method"
                        )));
                    }
                    if contract.has_runtime_main(db) {
                        return Err(directive_error(format!(
                            "{context}: contract `{contract_name}` defines a runtime `main`, so selector dispatch is disabled"
                        )));
                    }
                    if !has_dispatch_runtime {
                        return Err(directive_error(format!(
                            "{context}: specialization did not emit the generated selector dispatcher"
                        )));
                    }

                    let def = function.def_id_value(db);
                    let mut methods = surface.methods.iter().filter(|method| method.def == def);
                    let method = match (methods.next(), methods.next()) {
                        (Some(method), None) => method,
                        (None, _) => {
                            return Err(directive_error(format!(
                                "{context}: typed dispatch metadata has no matching source DefId"
                            )));
                        }
                        (Some(_), Some(_)) => {
                            return Err(directive_error(format!(
                                "{context}: typed dispatch metadata is ambiguous for its source DefId"
                            )));
                        }
                    };

                    match &selected_contract {
                        None => selected_contract = Some(contract_name.clone()),
                        Some(selected) if selected == &contract_name => {}
                        Some(selected) => {
                            return Err(directive_error(format!(
                                "one fixture may target only one deployed contract; found `{selected}` and `{contract_name}`"
                            )));
                        }
                    }
                    let inputs = abi_shapes(&method.inputs);
                    let outputs = abi_shapes(&method.outputs);
                    calls.extend(resolve_e2e_comments(
                        method.signature.clone(),
                        method.selector.0,
                        &inputs,
                        &outputs,
                        comments.iter().copied(),
                    )?);
                }
            }
            _ => {}
        }
    }

    let contract = selected_contract.ok_or_else(|| {
        directive_error("fixture contains no E2E directives on selector-dispatched methods")
    })?;
    if calls.is_empty() {
        return Err(directive_error(
            "fixture contains no executable E2E directives",
        ));
    }
    Ok(ResolvedFixtureDirectives { contract, calls })
}

fn reject_non_dispatch_directives(
    db: &'static TestDb,
    function: FunctionDef<'static>,
    kind: &str,
) -> Result<(), E2eFailure> {
    let name = function_name(db, function);
    let context = format!("{kind} `{name}`");
    let comments = comment_texts(db, function);
    if contains_directive(&comments, &context)? {
        return Err(directive_error(format!(
            "{context}: directives require a public contract selector method"
        )));
    }
    Ok(())
}

fn comment_texts(db: &'static TestDb, function: FunctionDef<'static>) -> Vec<&'static str> {
    function
        .leading_comments(db)
        .iter()
        .map(|comment| comment.text.as_str())
        .collect()
}

fn contains_directive(comments: &[&str], context: &str) -> Result<bool, E2eFailure> {
    let mut found = false;
    for comment in comments {
        match parse_e2e_directive(comment) {
            Ok(Some(_)) => found = true,
            Ok(None) => {}
            Err(error) => {
                return Err(directive_error(format!("{context}: {error}")));
            }
        }
    }
    Ok(found)
}

fn abi_shapes(params: &[AbiParam]) -> Vec<AbiShape> {
    params.iter().map(abi_shape).collect()
}

fn abi_shape(param: &AbiParam) -> AbiShape {
    match &param.ty {
        AbiType::Uint256 => AbiShape::Word,
        AbiType::Bool => AbiShape::Bool,
        AbiType::Unit => AbiShape::Unit,
        AbiType::Tuple => AbiShape::Tuple(abi_shapes(&param.components)),
        AbiType::Named(name) => match name.as_str() {
            "uint" | "uint256" | "word" => AbiShape::Word,
            "bool" => AbiShape::Bool,
            "address" => AbiShape::Address,
            "bytes32" => AbiShape::Bytes32,
            _ => AbiShape::Unsupported(name.clone()),
        },
        AbiType::String => AbiShape::Unsupported("string".to_owned()),
        AbiType::Unsupported => AbiShape::Unsupported(param.ty.to_string()),
    }
}

fn contract_program(
    program: &Program<'static>,
    contract: &str,
) -> Result<Program<'static>, E2eFailure> {
    let deployer = format!("{contract}Deploy");
    let mut objects = program
        .objects
        .iter()
        .filter(|object| object.name.as_str() == deployer);
    let object = match (objects.next(), objects.next()) {
        (Some(object), None) => object.clone(),
        (None, _) => {
            return Err(pipeline_error(format!(
                "Hull deploy object `{deployer}` was not emitted"
            )));
        }
        (Some(_), Some(_)) => {
            return Err(pipeline_error(format!(
                "Hull deploy object `{deployer}` is ambiguous"
            )));
        }
    };
    Ok(Program {
        span: program.span,
        entry_points: Vec::new(),
        functions: Vec::new(),
        objects: vec![object],
    })
}

fn function_name(db: &'static TestDb, function: FunctionDef<'static>) -> String {
    function.sig(db).name.atom().text(db).to_owned()
}

fn directive_error(message: impl Into<String>) -> E2eFailure {
    E2eFailure::new(FailureKind::Directive, message)
}

fn pipeline_error(message: impl Into<String>) -> E2eFailure {
    E2eFailure::new(FailureKind::Pipeline, message)
}

fn repo_root() -> PathBuf {
    repo_root_from_manifest(env!("CARGO_MANIFEST_DIR"))
}

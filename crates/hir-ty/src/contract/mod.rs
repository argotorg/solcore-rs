//! Contract-specific typed surfaces and frontend desugar planning.
//!
//! This module intentionally lives in `hir-ty`, not a new `hir-lower` crate:
//! dispatch eligibility, ABI spelling, duplicate public signatures, and field
//! initializer checks all need resolved names and lowered semantic types. The
//! later Hull/codegen stages can consume the typed surface and storage hooks
//! without re-deriving frontend rules from raw HIR.

mod abi;
mod abi_json;
mod desugar;
mod dispatch;
mod helpers;

pub use abi::{AbiParam, AbiSelector, AbiSignature, AbiType, abi_selector};
pub use abi_json::contract_abi_json;
pub use desugar::{
    BodyDesugarPlan, BoolNode, FrontendDesugarPlan, FrontendTransform, IndirectArgShape,
    frontend_desugar_plan,
};
pub(crate) use dispatch::module_manual_generic_abi_diagnostics;
pub use dispatch::{
    DispatchConstructor, DispatchFallback, DispatchMethod, DispatchSurface,
    contract_dispatch_surface, contract_dispatch_surface_for_module,
    contract_needs_generated_dispatch, module_contract_diagnostics,
};

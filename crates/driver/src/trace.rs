use std::env;

use tracing_subscriber::EnvFilter;

const TRACE_DEFAULT_FILTER: &str = concat!(
    "warn,",
    "driver::modules=debug,",
    "compiler::pipeline=debug,compiler::abi=debug,",
    "parser=debug,parser::query=debug,parser::recovery=trace,",
    "hir::query=debug,",
    "hir_ty::frontend=debug,hir_ty::query=debug,",
    "nameres=debug,nameres::query=debug,nameres::imports=trace,nameres::fixpoint=debug,",
    "driver::salsa=debug"
);

pub(crate) fn init_tracing(trace: bool) {
    let has_rust_log = env::var_os("RUST_LOG").is_some();
    if !trace && !has_rust_log {
        return;
    }

    let filter = if has_rust_log {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(TRACE_DEFAULT_FILTER))
    } else {
        EnvFilter::new(TRACE_DEFAULT_FILTER)
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .compact()
        .init();
}

pub(crate) fn emit_salsa_event(event: salsa::Event) {
    match event.kind {
        salsa::EventKind::WillExecute { database_key } => {
            tracing::debug!(
                target: "driver::salsa",
                event = "WillExecute",
                thread = ?event.thread_id,
                key = ?database_key,
                "salsa query will execute"
            );
        }
        salsa::EventKind::DidValidateMemoizedValue { database_key } => {
            tracing::debug!(
                target: "driver::salsa",
                event = "DidValidateMemoizedValue",
                thread = ?event.thread_id,
                key = ?database_key,
                "salsa memoized value validated"
            );
        }
        salsa::EventKind::DidValidateInternedValue { key, revision } => {
            tracing::trace!(
                target: "driver::salsa",
                event = "DidValidateInternedValue",
                thread = ?event.thread_id,
                key = ?key,
                revision = ?revision,
                "salsa interned value validated"
            );
        }
        salsa::EventKind::WillIterateCycle {
            database_key,
            iteration,
        } => {
            tracing::debug!(
                target: "driver::salsa",
                event = "WillIterateCycle",
                thread = ?event.thread_id,
                key = ?database_key,
                iteration,
                "salsa cycle will iterate"
            );
        }
        salsa::EventKind::DidFinalizeCycle {
            database_key,
            iteration,
        } => {
            tracing::debug!(
                target: "driver::salsa",
                event = "DidFinalizeCycle",
                thread = ?event.thread_id,
                key = ?database_key,
                iteration,
                "salsa cycle finalized"
            );
        }
        kind => {
            tracing::trace!(
                target: "driver::salsa",
                thread = ?event.thread_id,
                kind = ?kind,
                "salsa event"
            );
        }
    }
}

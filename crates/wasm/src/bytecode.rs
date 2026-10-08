//! Generated EVM sections for the playground's bytecode viewer.

use revm::primitives::hex;
use serde::Serialize;
use vfs::AnalysisHost;

use crate::backend::{self, Backend};

#[derive(Clone, Serialize)]
pub(crate) struct Object {
    pub(crate) name: String,
    pub(crate) sections: Vec<Section>,
}

#[derive(Clone, Serialize)]
pub(crate) struct Section {
    pub(crate) name: String,
    pub(crate) code: String,
}

pub(crate) fn generate(
    backend: Backend,
    db: &AnalysisHost,
    program: &hull::Program<'_>,
) -> Result<Vec<Object>, String> {
    let objects = backend::compile_program(backend, db, program)?;
    // An object-less main runs directly; its init section is not creation code.
    let with_init = !program.objects.is_empty();
    Ok(objects
        .into_iter()
        .map(|object| {
            let mut sections = Vec::new();
            if with_init {
                sections.push(section("init", &object.init));
            }
            sections.push(section("runtime", &object.runtime));
            Object {
                name: object.name,
                sections,
            }
        })
        .collect())
}

fn section(name: &str, bytes: &[u8]) -> Section {
    Section {
        name: name.to_owned(),
        code: format!("0x{}", hex::encode(bytes)),
    }
}

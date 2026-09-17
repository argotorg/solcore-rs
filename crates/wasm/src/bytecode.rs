//! Generated EVM sections for the playground's bytecode viewer.

use revm::primitives::hex;
use serde::Serialize;
use sonatina_codegen::{EvmCompile, OptLevel};
use vfs::AnalysisHost;

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
    db: &AnalysisHost,
    program: &hull::Program<'_>,
) -> Result<Vec<Object>, String> {
    let module = sonatina::translate_hull_program(db, program)
        .map_err(|err| format!("Sonatina translation failed: {err}"))?;
    let artifacts = EvmCompile::new(module)
        .with_opt_level(OptLevel::O0)
        .compile()
        .map_err(|errors| format!("EVM bytecode generation failed: {errors:?}"))?;
    Ok(artifacts
        .into_iter()
        .map(|artifact| Object {
            name: artifact.object.0.to_string(),
            sections: artifact
                .sections
                .into_iter()
                // An object-less main runs directly; its init section is not creation code.
                .filter(|(name, _)| !program.objects.is_empty() || name.0 != "init")
                .map(|(name, section)| Section {
                    name: name.0.to_string(),
                    code: format!("0x{}", hex::encode(section.bytes)),
                })
                .collect(),
        })
        .collect())
}

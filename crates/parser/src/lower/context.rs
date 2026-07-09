use hir::{
    anchor::{DefId, DefKind, DefLocation, KeyCanonicalizer},
    input::SourceFile,
};

use super::span::offset_from_usize;
use crate::{Db, types::ParsedError};

pub(super) struct LoweringCtx<'db, 'a> {
    pub(super) db: &'db dyn Db,
    pub(super) file: SourceFile,
    owner: Option<DefId<'db>>,
    keys: &'a mut KeyCanonicalizer,
    def_locations: &'a mut Vec<(DefId<'db>, DefLocation)>,
    pub(super) source: &'a str,
    pub(super) parse_errors: &'a mut Vec<ParsedError>,
}

impl<'db, 'a> LoweringCtx<'db, 'a> {
    pub(super) fn new(
        db: &'db dyn Db,
        file: SourceFile,
        owner: Option<DefId<'db>>,
        keys: &'a mut KeyCanonicalizer,
        def_locations: &'a mut Vec<(DefId<'db>, DefLocation)>,
        source: &'a str,
        parse_errors: &'a mut Vec<ParsedError>,
    ) -> Self {
        Self {
            db,
            file,
            owner,
            keys,
            def_locations,
            source,
            parse_errors,
        }
    }

    pub(super) fn with_owner<T>(&mut self, owner: DefId<'db>, f: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.owner.replace(owner);
        let result = f(self);
        self.owner = previous;
        result
    }

    pub(super) fn alloc_def_with_location(
        &mut self,
        kind: DefKind,
        name: Option<&str>,
        base_start: usize,
    ) -> DefId<'db> {
        self.alloc_def_with_fingerprint(kind, name, None, base_start)
    }

    pub(super) fn alloc_def_with_fingerprint(
        &mut self,
        kind: DefKind,
        name: Option<&str>,
        fingerprint: Option<&str>,
        base_start: usize,
    ) -> DefId<'db> {
        let def = self
            .keys
            .alloc_def(self.db, self.file, self.owner, kind, name, fingerprint);
        self.def_locations.push((
            def,
            DefLocation {
                file: self.file,
                base_offset: offset_from_usize(base_start),
            },
        ));
        def
    }
}

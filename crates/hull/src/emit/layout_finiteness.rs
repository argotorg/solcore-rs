//! Decide whether expanding ADT representations creates finitely many type
//! instantiations. A recursive parameter flow may permute or discard arguments;
//! it becomes infinite only when a cycle also wraps an argument in a constructor
//! or qualifier retained by the semantic type keys used during expansion.
//!
//! Arguments are lazy substitutions carrying their caller's expansion path.
//! Expanding `Box<Box<a>>` therefore consumes a finite input type instead of
//! mistaking both occurrences of `Box` for a recursive definition. Conversely,
//! `Nest<Box<a>>` in Nest's own representation retains the Nest recursion edge.

use std::{collections::HashMap, rc::Rc};

use super::*;

#[derive(Default)]
pub(super) struct LayoutFiniteness<'db> {
    checked: HashMap<DefId<'db>, bool>,
}

#[derive(Clone)]
struct Argument<'db> {
    ty: SemTy<'db>,
    env: Option<Rc<Vec<Argument<'db>>>>,
    path: Vec<DefId<'db>>,
}

struct Analysis<'a, 'db> {
    db: &'db dyn hir_ty::Db,
    root: DefId<'db>,
    representation: &'a dyn Fn(DefId<'db>) -> Option<SemTy<'db>>,
    flows: Vec<Vec<(usize, bool)>>,
    reached: Vec<(DefId<'db>, usize)>,
}

impl<'db> LayoutFiniteness<'db> {
    pub(super) fn is_finite(
        &mut self,
        db: &'db dyn hir_ty::Db,
        root: DefId<'db>,
        arity: usize,
    ) -> bool {
        self.check(db, root, arity, &|def| {
            let module = parse_file_to_hir(db, def.file(db)).module(db);
            let adt = super::layout::find_adt(db, module, def)?;
            Some(hir_ty::derived_generic_plan(db, module, adt)?.rep)
        })
    }

    fn check(
        &mut self,
        db: &'db dyn hir_ty::Db,
        root: DefId<'db>,
        arity: usize,
        representation: &dyn Fn(DefId<'db>) -> Option<SemTy<'db>>,
    ) -> bool {
        if let Some(finite) = self.checked.get(&root) {
            return *finite;
        }
        let mut pending = vec![(root, arity)];
        let mut seen = Vec::new();
        while let Some((def, arity)) = pending.pop() {
            if seen.contains(&def) {
                continue;
            }
            if let Some(finite) = self.checked.get(&def) {
                if !finite {
                    self.checked.insert(root, false);
                    return false;
                }
                continue;
            }
            seen.push(def);
            let mut analysis = Analysis {
                db,
                root: def,
                representation,
                flows: vec![Vec::new(); arity],
                reached: Vec::new(),
            };
            let args = (0..arity)
                .map(|index| Argument {
                    ty: SemTy::bound(db, index as u32),
                    env: None,
                    path: Vec::new(),
                })
                .collect();
            analysis.definition(def, Rc::new(args), vec![def]);
            if analysis.has_growing_cycle() {
                self.checked.insert(def, false);
                self.checked.insert(root, false);
                return false;
            }
            pending.extend(analysis.reached);
        }
        for def in seen {
            self.checked.insert(def, true);
        }
        true
    }
}

impl<'db> Analysis<'_, 'db> {
    fn definition(&mut self, def: DefId<'db>, env: Rc<Vec<Argument<'db>>>, path: Vec<DefId<'db>>) {
        if let Some(rep) = (self.representation)(def) {
            self.ty(rep, Some(&env), &path);
        }
    }

    fn ty(&mut self, ty: SemTy<'db>, env: Option<&Rc<Vec<Argument<'db>>>>, path: &[DefId<'db>]) {
        match ty.kind(self.db) {
            SemTyKind::BoundVar(var) => {
                if let Some(arg) = env.and_then(|env| env.get(var.index as usize)) {
                    self.ty(arg.ty, arg.env.as_ref(), &arg.path);
                }
            }
            SemTyKind::Named {
                ctor: TyCtor::User(user),
                args,
            } if user.kind == UserTyCtorKind::Adt => {
                self.reached.push((user.def, args.len()));
                if path.contains(&user.def) {
                    if user.def == self.root {
                        for (target, arg) in args.iter().enumerate() {
                            self.argument_flows(*arg, env, target, false);
                        }
                    }
                    return;
                }
                let args = args
                    .iter()
                    .map(|arg| Argument {
                        ty: *arg,
                        env: env.cloned(),
                        path: path.to_vec(),
                    })
                    .collect();
                let mut inner = path.to_vec();
                inner.push(user.def);
                self.definition(user.def, Rc::new(args), inner);
            }
            SemTyKind::Named { args, .. } | SemTyKind::Tuple(args) => {
                for arg in args {
                    self.ty(*arg, env, path);
                }
            }
            SemTyKind::Function { params, ret } => {
                for param in params {
                    self.ty(*param, env, path);
                }
                self.ty(*ret, env, path);
            }
            SemTyKind::Comptime(inner) => self.ty(*inner, env, path),
            SemTyKind::Error | SemTyKind::Unknown => {}
        }
    }

    fn argument_flows(
        &mut self,
        ty: SemTy<'db>,
        env: Option<&Rc<Vec<Argument<'db>>>>,
        target: usize,
        wrapped: bool,
    ) {
        match ty.kind(self.db) {
            SemTyKind::BoundVar(var) => match env {
                Some(env) => {
                    if let Some(arg) = env.get(var.index as usize) {
                        self.argument_flows(arg.ty, arg.env.as_ref(), target, wrapped);
                    }
                }
                None => {
                    if let Some(edges) = self.flows.get_mut(var.index as usize) {
                        edges.push((target, wrapped));
                    }
                }
            },
            SemTyKind::Named { args, .. } | SemTyKind::Tuple(args) => {
                for arg in args {
                    self.argument_flows(*arg, env, target, true);
                }
            }
            SemTyKind::Function { params, ret } => {
                for param in params {
                    self.argument_flows(*param, env, target, true);
                }
                self.argument_flows(*ret, env, target, true);
            }
            // Lowering erases a top-level comptime qualifier, but an ADT's
            // argument is substituted and keyed before that lowering occurs.
            // Repeatedly wrapping the argument therefore grows the expansion
            // just like any other constructor retained in its semantic key.
            SemTyKind::Comptime(inner) => self.argument_flows(*inner, env, target, true),
            SemTyKind::Error | SemTyKind::Unknown => {}
        }
    }

    fn has_growing_cycle(&self) -> bool {
        for (source, edges) in self.flows.iter().enumerate() {
            for &(target, wrapped) in edges {
                if !wrapped {
                    continue;
                }
                let mut pending = vec![target];
                let mut seen = vec![false; self.flows.len()];
                while let Some(node) = pending.pop() {
                    if node == source {
                        return true;
                    }
                    if !seen[node] {
                        seen[node] = true;
                        pending.extend(self.flows[node].iter().map(|&(target, _)| target));
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hir_ty::UserTyCtor;
    use nameres::{Db as _, module_id_from_key};
    use solcore_test_utils::{define_frontend_test_db, load_main_source};

    define_frontend_test_db!(TestDb, hir_ty);

    #[test]
    fn mutual_representation_cycles_detect_growth_and_allow_regular_recursion() {
        let mut db = TestDb::default();
        // Supply resolved representation plans directly: source mutual ADTs
        // currently fail name resolution before reaching layout lowering.
        let key = load_main_source(
            &mut db,
            "pragma solcore; enum A<a> { A(a) } enum B<b> { B(b) } enum Box<a> { Box(a) }",
        );
        let entry = module_id_from_key(&db, &key);
        let file = db.module_file(entry).unwrap();
        let module = parse_file_to_hir(&db, file).module(&db);
        let defs = module
            .items(&db)
            .iter()
            .filter_map(|item| match item {
                Item::AdtDef(adt) => Some(adt.def_id_value(&db)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let named = |def, arg| {
            SemTy::named(
                &db,
                TyCtor::User(UserTyCtor {
                    def,
                    kind: UserTyCtorKind::Adt,
                }),
                vec![arg],
            )
        };
        let param = SemTy::bound(&db, 0);
        for argument in [param, named(defs[2], param), SemTy::comptime(&db, param)] {
            let plans = HashMap::from([
                (defs[0], named(defs[1], param)),
                (defs[1], named(defs[0], argument)),
                (defs[2], param),
            ]);
            for root in [defs[0], defs[1]] {
                assert_eq!(
                    LayoutFiniteness::default()
                        .check(&db, root, 1, &|def| plans.get(&def).copied()),
                    argument == param
                );
            }
        }
    }
}

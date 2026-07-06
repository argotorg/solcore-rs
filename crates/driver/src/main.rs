use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
};

use hir::{diag::Diagnostic, input::SourceFile};
use nameres::{
    LibraryId, ModuleId, ModuleKey, ModuleTree, module_id_from_key, module_key_for_path,
    resolve_module_path_candidate, resolve_reachable_full,
};
use parser::parse_file_to_hir;
use url::Url;

#[salsa::db]
#[derive(Clone, Default)]
struct DriverDb {
    storage: salsa::Storage<Self>,
    module_tree: Option<ModuleTree>,
    module_files: HashMap<ModuleKey, SourceFile>,
}

#[salsa::db]
impl salsa::Database for DriverDb {}

#[salsa::db]
impl hir::Db for DriverDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl parser::Db for DriverDb {}

#[salsa::db]
impl nameres::Db for DriverDb {
    fn module_tree(&self) -> ModuleTree {
        self.module_tree
            .expect("DriverDb module tree is initialized before use")
    }

    fn module_file<'db>(&'db self, module: ModuleId<'db>) -> Option<SourceFile> {
        self.module_files.get(&module.key(self)).copied()
    }
}

fn main() {
    let program = env::args()
        .next()
        .unwrap_or_else(|| "solcore-driver".to_owned());
    let args = match parse_args(env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("usage: {program} [--external-lib NAME=PATH] <input.solc>");
            std::process::exit(2);
        }
    };

    let input_path = match absolutize(&args.input) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve `{}`: {err}", args.input.display());
            std::process::exit(1);
        }
    };
    let source = match fs::read_to_string(&input_path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("failed to read `{}`: {err}", input_path.display());
            std::process::exit(1);
        }
    };

    let main_root = input_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let std_root = repo_root().join("std");
    let external_roots = args
        .external_roots
        .into_iter()
        .map(|(name, path)| {
            absolutize(&path)
                .map(|path| (name, path))
                .map_err(|err| format!("failed to resolve `{}`: {err}", path.display()))
        })
        .collect::<Result<BTreeMap<_, _>, _>>();
    let external_roots = match external_roots {
        Ok(roots) => roots,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };

    let mut db = DriverDb::default();
    db.module_tree = Some(ModuleTree::new(
        &db,
        main_root.clone(),
        std_root,
        external_roots,
    ));

    let entry_key = match module_key_for_path(LibraryId::Main, &main_root, &input_path) {
        Some(key) => key,
        None => {
            eprintln!(
                "source file `{}` is outside module root `{}`",
                input_path.display(),
                main_root.display()
            );
            std::process::exit(1);
        }
    };
    let entry_file = match source_file_for_path(&db, &input_path, source) {
        Ok(file) => file,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    db.module_files.insert(entry_key.clone(), entry_file);

    load_reachable_modules(&mut db, entry_key.clone());

    let entry = module_id_from_key(&db, &entry_key);
    let _ = resolve_reachable_full(&db, entry);
    let diagnostics = resolve_reachable_full::accumulated::<Diagnostic>(&db, entry);
    if diagnostics.is_empty() {
        return;
    }

    for diagnostic in diagnostics {
        eprint!("{}", diagnostic.render(&db));
    }
    std::process::exit(1);
}

struct Args {
    input: PathBuf,
    external_roots: Vec<(String, PathBuf)>,
}

fn parse_args(args: Vec<String>) -> Result<Args, String> {
    let mut input = None;
    let mut external_roots = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--external-lib" | "--lib" => {
                let Some(value) = iter.next() else {
                    return Err(format!("{arg} requires NAME=PATH"));
                };
                external_roots.push(parse_external_root(&value)?);
            }
            _ if arg.starts_with("--external-lib=") => {
                external_roots.push(parse_external_root(&arg["--external-lib=".len()..])?);
            }
            _ if arg.starts_with("--lib=") => {
                external_roots.push(parse_external_root(&arg["--lib=".len()..])?);
            }
            _ if arg.starts_with('-') => {
                return Err(format!("unknown option `{arg}`"));
            }
            _ => {
                if input.replace(PathBuf::from(&arg)).is_some() {
                    return Err("expected exactly one input file".to_owned());
                }
            }
        }
    }

    let Some(input) = input else {
        return Err("missing input file".to_owned());
    };
    Ok(Args {
        input,
        external_roots,
    })
}

fn parse_external_root(value: &str) -> Result<(String, PathBuf), String> {
    let Some((name, path)) = value.split_once('=') else {
        return Err(format!("external library must be NAME=PATH, got `{value}`"));
    };
    if name.is_empty() || path.is_empty() {
        return Err(format!("external library must be NAME=PATH, got `{value}`"));
    }
    Ok((name.to_owned(), PathBuf::from(path)))
}

fn load_reachable_modules(db: &mut DriverDb, entry: ModuleKey) {
    let mut queue = VecDeque::from([entry]);
    let mut visited = HashSet::new();

    while let Some(key) = queue.pop_front() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(file) = db.module_files.get(&key).copied() else {
            continue;
        };
        let targets = {
            let module = module_id_from_key(&*db, &key);
            let refs = nameres::module_imports(&*db, file);
            refs.import_refs
                .into_iter()
                .chain(refs.export_refs)
                .filter_map(|path| {
                    let resolved = resolve_module_path_candidate(&*db, module, &path).ok()?;
                    Some((resolved.module.key(&*db), resolved.file_path))
                })
                .collect::<Vec<_>>()
        };
        for (target_key, file_path) in targets {
            if !db.module_files.contains_key(&target_key)
                && let Ok(source) = fs::read_to_string(&file_path)
                && let Ok(file) = source_file_for_path(db, &file_path, source)
            {
                db.module_files.insert(target_key.clone(), file);
            }
            if db.module_files.contains_key(&target_key) {
                queue.push_back(target_key);
            }
        }
    }
}

fn source_file_for_path(db: &DriverDb, path: &Path, source: String) -> Result<SourceFile, String> {
    let url = Url::from_file_path(path)
        .map_err(|()| format!("failed to convert `{}` into file URL", path.display()))?;
    Ok(SourceFile::new(db, url, Some(source)))
}

fn absolutize(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir().map(|cwd| cwd.join(path))
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("driver crate lives under <repo>/crates/driver")
        .to_path_buf()
}

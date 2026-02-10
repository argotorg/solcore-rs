use std::{env, fs, path::Path};

use hull::{diag::Diagnostic, input::SourceFile};
use parser::parse_file_to_hull;
use url::Url;

#[salsa::db]
#[derive(Default, Clone)]
struct DriverDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for DriverDb {}

#[salsa::db]
impl hull::Db for DriverDb {}

#[salsa::db]
impl parser::Db for DriverDb {}

fn main() {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "solcore-driver".to_owned());
    let Some(path_arg) = args.next() else {
        eprintln!("usage: {program} <input.solc>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: {program} <input.solc>");
        std::process::exit(2);
    }

    let path = Path::new(&path_arg);
    let canonical_path = match path.canonicalize() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve `{}`: {err}", path.display());
            std::process::exit(1);
        }
    };

    let source = match fs::read_to_string(&canonical_path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("failed to read `{}`: {err}", canonical_path.display());
            std::process::exit(1);
        }
    };

    let url = match Url::from_file_path(&canonical_path) {
        Ok(url) => url,
        Err(()) => {
            eprintln!(
                "failed to convert `{}` into file URL",
                canonical_path.display()
            );
            std::process::exit(1);
        }
    };

    let db = DriverDb::default();
    let file = SourceFile::new(&db, url, Some(source));
    let _ = parse_file_to_hull(&db, file).module(&db);

    let diagnostics = parse_file_to_hull::accumulated::<Diagnostic>(&db, file);
    if diagnostics.is_empty() {
        return;
    }

    for diag in diagnostics {
        eprint!("{}", diag.render(&db));
    }
    std::process::exit(1);
}

use cairo_lang_compiler::db::RootDatabase;
use cairo_lang_defs::db::DefsGroup;
use cairo_lang_defs::ids::ModuleId;
use cairo_lang_diagnostics::{DiagnosticEntry, Diagnostics};
use cairo_lang_filesystem::db::FilesGroup;
use cairo_lang_filesystem::ids::{FileId, FileLongId};
use cairo_lang_lowering::db::LoweringGroup;
use cairo_lang_parser::db::ParserGroup;
use cairo_lang_semantic::db::SemanticGroup;
use cairo_lang_utils::{LookupIntern, Upcast};
use std::collections::{HashSet, VecDeque};
use std::iter;
use std::iter::zip;
use std::num::NonZero;

mod pool;

use crate::diagnostics::pool::Pool;

pub struct DiagnosticController {
    pool: Pool,
}

impl DiagnosticController {
    pub fn new(threads_limit: NonZero<usize>) -> Self {
        Self {
            pool: Pool::new(threads_limit.get()),
        }
    }

    pub fn calculate_diagnostics_for_all_files(&self, db: &RootDatabase) {
        let files = find_all_files_from_all_crates(db);
        let files_batches = batches(&files, self.pool.parallelism());

        let db_snapshots = iter::from_fn(|| Some(salsa::Snapshot::new(db.snapshot())))
            .take(self.pool.parallelism().get())
            .collect();

        self.spawn_refresh_workers(files_batches, db_snapshots);
    }

    fn spawn_refresh_workers(
        &self,
        files_batches: Vec<Vec<FileId>>,
        db_snapshots: Vec<salsa::Snapshot<RootDatabase>>,
    ) {
        assert_eq!(files_batches.len(), db_snapshots.len());
        for (batch, snapshot) in zip(files_batches, db_snapshots) {
            self.pool.spawn(move || {
                for file in batch {
                    calculate_diags_for_file(&snapshot, file);
                }
            });
        }
    }
}

fn find_all_files_from_all_crates(db: &RootDatabase) -> Vec<FileId> {
    let mut result = HashSet::new();
    for crate_id in db.crates() {
        for module_id in db.crate_modules(crate_id).iter() {
            // Schedule only on disk module main files for refreshing.
            // All other related files will be refreshed along with it in a single job.
            if let Ok(file) = db.module_main_file(*module_id) {
                if matches!(file.lookup_intern(db), FileLongId::OnDisk(_)) {
                    result.insert(file);
                }
            }
        }
    }
    result.into_iter().collect()
}

fn batches(input: &[FileId], n: NonZero<usize>) -> Vec<Vec<FileId>> {
    let n = n.get();
    (1..=n)
        .map(|offset| input.iter().copied().skip(offset - 1).step_by(n).collect())
        .collect()
}

/// Calculates all diagnostics kinds by processing an on disk `root_on_disk_file` together with
/// virtual files that are its descendants.
fn calculate_diags_for_file(db: &RootDatabase, root_on_disk_file: FileId) {
    let Some((files_to_process, modules_to_process)) =
        file_and_subfiles_with_corresponding_modules(db, root_on_disk_file)
    else {
        eprintln!("error during diagnostics calculation");
        return;
    };

    fn print_diags<T: DiagnosticEntry>(db: &<T as DiagnosticEntry>::DbType, diags: Diagnostics<T>) {
        for entry in diags.format_with_severity(db, &Default::default()) {
            if !entry.is_empty() {
                eprint!("{entry}")
            }
        }
    }

    for module_id in modules_to_process.into_iter() {
        let diags = db
            .module_semantic_diagnostics(module_id)
            .unwrap_or_default();
        print_diags(db.upcast(), diags);

        let diags = db
            .module_lowering_diagnostics(module_id)
            .unwrap_or_default();
        print_diags(db.upcast(), diags);
    }

    for file_id in files_to_process.into_iter() {
        let diags = db.file_syntax_diagnostics(file_id);
        print_diags(db.upcast(), diags);
    }
}

/// **DISCLAIMER**: this is a query in LS.
///
/// Collects `file` and all its descendants together with modules from all these files.
///
/// **CAVEAT**: it does not collect descendant files that come from inline macros - it will when
/// the compiler moves inline macros resolving to [`DefsGroup`].
fn file_and_subfiles_with_corresponding_modules(
    db: &dyn SemanticGroup,
    file: FileId,
) -> Option<(HashSet<FileId>, HashSet<ModuleId>)> {
    let mut modules: HashSet<_> = db.file_modules(file).ok()?.iter().copied().collect();
    let mut files = HashSet::from([file]);
    // Collect descendants of `file`
    // and modules from all virtual files that are descendants of `file`.
    //
    // Caveat: consider a situation `file1` --(child)--> `file2` with file contents:
    // - `file1`: `mod file2_origin_module { #[file2]fn sth() {} }`
    // - `file2`: `mod mod_from_file2 { }`
    //  It is important that `file2` content contains a module.
    //
    // Problem: in this situation it is not enough to call `db.file_modules(file1_id)` since
    //  `mod_from_file2` won't be in the result of this query.
    // Solution: we can find file id of `file2`
    //  (note that we only have file id of `file1` at this point)
    //  in `db.module_files(mod_from_file1_from_which_file2_origins)`.
    //  Then we can call `db.file_modules(file2_id)` to obtain module id of `mod_from_file2`.
    //  We repeat this procedure until there is nothing more to collect.
    let mut modules_queue: VecDeque<_> = modules.iter().copied().collect();
    while let Some(module_id) = modules_queue.pop_front() {
        for file_id in db.module_files(module_id).ok()?.iter() {
            if files.insert(*file_id) {
                for module_id in db.file_modules(*file_id).ok()?.iter() {
                    if modules.insert(*module_id) {
                        modules_queue.push_back(*module_id);
                    }
                }
            }
        }
    }
    Some((files, modules))
}

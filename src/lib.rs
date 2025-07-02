use cairo_lang_compiler::db::RootDatabase;
use scarb_metadata::MetadataCommand;
use std::num::NonZero;
use std::path::PathBuf;

use crate::diagnostics::DiagnosticController;
use crate::project::extract_crates;

mod diagnostics;
mod project;

/// Loads a Scarb project with Scarb.toml under `manifest_path`.
/// This function calls `scarb metadata` and extracts information about the project from it.
/// Then it uses the information to set appropriate inputs in a newly created db.
///
/// This simulates LS behaviour when opening a cairo file from a Scarb project for the first time.
pub fn load_scarb_project(manifest_path: PathBuf) -> anyhow::Result<RootDatabase> {
    let mut db = RootDatabase::empty();

    let metadata = MetadataCommand::new()
        .manifest_path(manifest_path)
        .inherit_stderr()
        .exec()?;
    let crates_to_load = extract_crates(&metadata);

    // eprintln!("updating crate roots from scarb metadata: {crates_to_load:#?}");

    for cr in crates_to_load {
        cr.apply(&mut db);
    }

    Ok(db)
}

/// Calculates diagnostics for all files from all crates loaded into the db.
///
/// It does so by creating a thread pool, then splitting all relevant files into `n` batches where
/// `n` is the number of threads in the thread pool.
/// The batches are then sent to the threads which calculate diagnostics for files in the batch.
///
/// **NOTE**: in LS additional measures are taken to make sure open files are processed first.
/// This mechanism was skipped here for clarity.
/// To learn more, check https://github.com/software-mansion/cairols/blob/7d7611e2369598a68a64d6528519817be71b5dd4/src/lang/diagnostics/mod.rs#L148.
pub fn calculate_diagnostics_for_all_files(db: &RootDatabase, threads_limit: NonZero<usize>) {
    let diag_controller = DiagnosticController::new(threads_limit);

    let now = std::time::Instant::now();

    diag_controller.calculate_diagnostics_for_all_files(db);

    // Drop to make sure all threads are joined.
    drop(diag_controller);

    let elapsed = now.elapsed();
    println!("Diagnostics calculation time: {elapsed:.2?}");
}

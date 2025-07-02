use clap::Parser;
use demo_ls::{calculate_diagnostics_for_all_files, load_scarb_project};
use std::num::NonZero;
use std::path::PathBuf;

#[derive(Parser, Clone, Debug)]
pub struct Args {
    /// A path to a Scarb.toml from the project.
    pub manifest_path: PathBuf,

    /// Maximum number of threads in the thread pool.
    /// A thread pool will spawn `min(threads_limit, available_parallelism)` threads.
    #[arg(long, short, default_value = "4")]
    pub threads_limit: NonZero<usize>,
}

fn main() -> anyhow::Result<()> {
    let Args {
        manifest_path,
        threads_limit,
    } = Args::parse();

    let db = load_scarb_project(manifest_path)?;

    // This simulates diagnostics calculation.
    // Mind that in LS scheduling is also done in the background.
    calculate_diagnostics_for_all_files(&db, threads_limit);

    // To skip waiting for the salsa drop at the end - annoying.
    std::mem::forget(db);

    Ok(())
}

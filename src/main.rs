use clap::Parser;
use demo_ls::{calculate_diagnostics_for_all_files, load_project, setup_database};
use std::path::PathBuf;

#[derive(Parser, Clone, Debug)]
pub struct Args {
    /// The Scarb project path.
    pub project_path: PathBuf,

    /// Path to a Scarb binary.
    #[arg(long)]
    pub scarb: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut db = setup_database();

    // This simulates what would happen when opening a file from a project for the first time.
    load_project(&mut db, args.project_path)?;

    let now = std::time::Instant::now();
    // This simulates diagnostics calculation.
    // Mind that in LS scheduling is also done in the background.
    calculate_diagnostics_for_all_files(&db);
    let elapsed = now.elapsed();
    println!("Diagnostics calculation time: {elapsed:.2?}");

    Ok(())
}

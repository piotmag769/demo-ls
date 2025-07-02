use crate::diagnostics::DiagnosticController;
use crate::project::extract_crates;
use cairo_lang_compiler::db::RootDatabase;
use cairo_lang_executable::plugin::executable_plugin_suite;
use cairo_lang_filesystem::ids::CrateId;
use cairo_lang_semantic::db::PluginSuiteInput;
use cairo_lang_semantic::inline_macros::get_default_plugin_suite;
use cairo_lang_semantic::plugin::PluginSuite;
use cairo_lang_test_plugin::test_plugin_suite;
use scarb_metadata::MetadataCommand;
use std::path::PathBuf;

mod diagnostics;
mod pool;
mod project;

pub fn setup_database() -> RootDatabase {
    let mut db = RootDatabase::empty();

    let core_plugin_suite = [
        get_default_plugin_suite(),
        // We add these in case someone wants to use LS while developing corelib.
        // Doesn't change anything here - it is just to reflect what LS does.
        test_plugin_suite(),
        executable_plugin_suite(),
    ]
    .into_iter()
    .fold(PluginSuite::default(), |mut acc, suite| {
        acc.add(suite);
        acc
    });
    let core_plugin_suite = db.intern_plugin_suite(core_plugin_suite);
    db.set_override_crate_plugins_from_suite(CrateId::core(&db), core_plugin_suite);

    db
}

pub fn load_project(db: &mut RootDatabase, project_path: PathBuf) -> anyhow::Result<()> {
    let metadata = MetadataCommand::new()
        .current_dir(project_path)
        .inherit_stderr()
        .exec()?;
    let crates_to_load = extract_crates(&metadata);

    eprintln!("updating crate roots from scarb metadata: {crates_to_load:#?}");

    for cr in crates_to_load {
        cr.apply(db);
    }

    Ok(())
}

pub fn calculate_diagnostics_for_all_files(db: &RootDatabase) {
    let diag_controller = DiagnosticController::new();
    diag_controller.calculate_diagnostics_for_all_files(db);
}

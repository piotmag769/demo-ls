use crate::project::plugins::BuiltinPlugin;
use cairo_lang_compiler::db::RootDatabase;
use cairo_lang_defs::db::DefsGroup;
use cairo_lang_defs::ids::ModuleId;
use cairo_lang_filesystem::db::{
    CORELIB_CRATE_NAME, CrateConfiguration, CrateSettings, FilesGroupEx,
};
use cairo_lang_filesystem::ids::{CrateId, CrateLongId, Directory};
use cairo_lang_semantic::db::PluginSuiteInput;
use cairo_lang_semantic::inline_macros::get_default_plugin_suite;
use cairo_lang_utils::Intern;
use cairo_lang_utils::smol_str::SmolStr;
use std::collections::HashSet;
use std::path::PathBuf;

/// A complete set of information needed to set up a real crate in the analysis database.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Crate {
    /// Crate name.
    pub name: SmolStr,

    /// Globally unique crate ID used for differentiating between crates with the same name.
    ///
    /// `None` is reserved for the core crate.
    pub discriminator: Option<SmolStr>,

    /// The root directory of the crate.
    ///
    /// This path **must** be absolute,
    /// so it can be safely used as a `FileId` in the analysis database.
    pub root: PathBuf,

    /// Custom stems of crate main files, if it is not `lib.cairo`.
    ///
    /// This is used to generate a virtual lib file for crates without a root `lib.cairo`.
    pub custom_main_file_stems: Option<Vec<SmolStr>>,

    /// Crate settings.
    pub settings: CrateSettings,

    /// Built-in plugins required by the crate.
    pub builtin_plugins: HashSet<BuiltinPlugin>,
}

impl Crate {
    /// Applies this crate to the [`AnalysisDatabase`].
    pub fn apply(&self, db: &mut RootDatabase) {
        assert!(
            (self.name == CORELIB_CRATE_NAME) ^ self.discriminator.is_some(),
            "invariant violation: only the `core` crate should have no discriminator"
        );

        let crate_id = CrateLongId::Real {
            name: self.name.clone(),
            discriminator: self.discriminator.clone(),
        }
        .intern(db);

        let crate_configuration = CrateConfiguration {
            root: Directory::Real(self.root.clone()),
            settings: self.settings.clone(),
            cache_file: None,
        };
        db.set_crate_config(crate_id, Some(crate_configuration));

        if let Some(file_stems) = &self.custom_main_file_stems {
            inject_virtual_wrapper_lib(db, crate_id, file_stems);
        }

        let plugins = self.builtin_plugins.iter().map(BuiltinPlugin::suite).fold(
            get_default_plugin_suite(),
            |mut acc, suite| {
                acc.add(suite);
                acc
            },
        );

        let interned_plugins = db.intern_plugin_suite(plugins);
        db.set_override_crate_plugins_from_suite(crate_id, interned_plugins);
    }
}

/// Generate a wrapper lib file for a compilation unit without a root `lib.cairo`.
///
/// This approach allows compiling crates that do not define `lib.cairo` file. For example, single
/// file crates can be created this way. The actual single file module is defined as `mod` item in
/// created lib file.
fn inject_virtual_wrapper_lib(db: &mut RootDatabase, crate_id: CrateId, file_stems: &[SmolStr]) {
    let module_id = ModuleId::CrateRoot(crate_id);
    let file_id = db.module_main_file(module_id).unwrap();

    let file_content = file_stems
        .iter()
        .map(|stem| format!("mod {stem};"))
        .collect::<Vec<_>>()
        .join("\n");

    // Inject a virtual lib file wrapper.
    db.override_file_content(file_id, Some(file_content.into()));
}

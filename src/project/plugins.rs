use cairo_lang_defs::plugin::{MacroPlugin, MacroPluginMetadata, PluginResult};
use cairo_lang_executable::plugin::executable_plugin_suite;
use cairo_lang_semantic::plugin::PluginSuite;
use cairo_lang_starknet::starknet_plugin_suite;
use cairo_lang_syntax::node::ast::ModuleItem;
use cairo_lang_syntax::node::db::SyntaxGroup;
use cairo_lang_test_plugin::{test_assert_suite, test_plugin_suite};
use scarb_metadata::{CompilationUnitCairoPluginMetadata, Metadata};

/// Representation of known built-in plugins available in the Cairo compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BuiltinPlugin {
    AssertMacros,
    Executable,
    CairoTest,
    Starknet,
    // It is normally handled with proc macros. It is there to prevent annoying diagnostics.
    SnforgeScarbPlugin,
}

impl BuiltinPlugin {
    /// Creates a new instance of `BuiltinPlugin` corresponding to the given
    /// [`CompilationUnitCairoPluginMetadata`].
    /// Returns `None` if `plugin_metadata` does not describe any known built-in plugin.
    pub fn from_plugin_metadata(
        metadata: &Metadata,
        plugin_metadata: &CompilationUnitCairoPluginMetadata,
    ) -> Option<Self> {
        // The package discriminator has form: "<name> <version> (<source>)".
        let package_id_repr = &plugin_metadata.package.repr;

        let package_metadata = metadata
            .packages
            .iter()
            .find(|package_metadata| &package_metadata.id.repr == package_id_repr)?;

        if package_metadata.name.contains("snforge_scarb_plugin") {
            return Some(Self::SnforgeScarbPlugin);
        }

        // Discard those plugins which are not built-in
        // before checking their discriminators in the next step.
        if !metadata
            .is_builtin_plugin(plugin_metadata)
            .unwrap_or_default()
        {
            return None;
        }

        match package_metadata.name.as_str() {
            "assert_macros" => Some(Self::AssertMacros),
            "cairo_execute" => Some(Self::Executable),
            "cairo_test" => Some(Self::CairoTest),
            "starknet" => Some(Self::Starknet),
            _ => None,
        }
    }

    /// Creates a [`PluginSuite`] corresponding to the represented plugin.
    pub fn suite(&self) -> PluginSuite {
        match self {
            BuiltinPlugin::AssertMacros => test_assert_suite(),
            BuiltinPlugin::CairoTest => test_plugin_suite(),
            BuiltinPlugin::Executable => executable_plugin_suite(),
            BuiltinPlugin::Starknet => starknet_plugin_suite(),
            BuiltinPlugin::SnforgeScarbPlugin => mock_snforge_scarb_plugin_suite(),
        }
    }
}

fn mock_snforge_scarb_plugin_suite() -> PluginSuite {
    let mut suite = PluginSuite::default();
    suite.add_plugin::<MockSnforgeScarbPlugin>();
    suite
}

#[derive(Debug, Default)]
pub struct MockSnforgeScarbPlugin;

impl MacroPlugin for MockSnforgeScarbPlugin {
    fn generate_code(
        &self,
        _db: &dyn SyntaxGroup,
        _item_ast: ModuleItem,
        _metadata: &MacroPluginMetadata<'_>,
    ) -> PluginResult {
        PluginResult {
            code: None,
            diagnostics: vec![],
            remove_original_item: false,
        }
    }

    fn declared_attributes(&self) -> Vec<String> {
        vec![
            "test".to_string(),
            "ignore".to_string(),
            "fuzzer".to_string(),
            "fork".to_string(),
            "available_gas".to_string(),
            "should_panic".to_string(),
            "disable_predeployed_contracts".to_string(),
        ]
    }
}

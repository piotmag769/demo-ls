use crate::project::crate_model::Crate;
use crate::project::plugins::BuiltinPlugin;
use anyhow::{Context, Result, bail, ensure};
use cairo_lang_filesystem::cfg::{Cfg, CfgSet};
use cairo_lang_filesystem::db::{
    CORELIB_CRATE_NAME, CrateSettings, DependencySettings, Edition, ExperimentalFeaturesConfig,
};
use cairo_lang_utils::OptionHelper;
use cairo_lang_utils::smol_str::ToSmolStr;
use itertools::Itertools;
use scarb_metadata::{
    CompilationUnitCairoPluginMetadata, CompilationUnitComponentDependencyMetadata,
    CompilationUnitComponentId, Metadata, PackageMetadata,
};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub mod crate_model;
pub mod plugins;

/// Extract information about crates that should be loaded to db from Scarb metadata.
///
/// This function attempts to be graceful. Any erroneous cases will be reported as warnings in logs.
///
/// In all real-world scenarios, this function should always extract info about the `core` crate.
/// Technically, it is possible for `scarb metadata` to omit `core` if working on a `no-core`
/// package, but in reality enabling `no-core` makes sense only for the `core` package itself. To
/// leave a trace of unreal cases, this function will log a warning if `core` is missing.
pub fn extract_crates(metadata: &Metadata) -> Vec<Crate> {
    // A crate can appear as a component in multiple compilation units.
    // We use a map here to make sure we include dependencies and cfg sets from all CUs.
    // We can keep components with assigned group id separately as they are not affected by this;
    // they are parts of integration tests crates which cannot appear in multiple compilation units.
    let mut crates_by_component_id: HashMap<CompilationUnitComponentId, Crate> = HashMap::new();
    let mut crates_grouped_by_group_id = HashMap::new();

    for compilation_unit in &metadata.compilation_units {
        if compilation_unit.target.kind == "cairo-plugin" {
            continue;
        }

        for component in &compilation_unit.components {
            let crate_name = component.name.as_str();
            let Some(component_id) = component.id.clone() else {
                eprintln!("id of component {crate_name} was None in metadata");
                continue;
            };

            let mut package = metadata
                .packages
                .iter()
                .find(|package| package.id == component.package);

            // For some compilation units corresponding to integration tests,
            // a main package of the compilation unit may not be found in a list of packages
            // (metadata hack, intended by scarb).
            // We instead find a package that specifies the target of the compilation unit
            // and later use it to extract edition and experimental features.
            if package.is_none() && compilation_unit.package == component.package {
                package = metadata.packages.iter().find(|p| {
                    p.targets
                        .iter()
                        .any(|t| t.name == compilation_unit.target.name)
                });
            }

            let Some(package) = package else {
                eprintln!("package for component is missing in scarb metadata: {crate_name}");
                continue;
            };

            let edition = scarb_package_edition(package, crate_name);
            let experimental_features = scarb_package_experimental_features(package);
            let version = Some(package.version.clone());

            let (root, file_stem) = match validate_and_chop_source_path(
                component.source_path.as_std_path(),
                crate_name,
            ) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{e:?}");
                    continue;
                }
            };
            let custom_main_file_stems = (file_stem != "lib").then_some(vec![file_stem.into()]);

            let cfg_set_from_scarb = scarb_cfg_set_to_cairo(
                component.cfg.as_ref().unwrap_or(&compilation_unit.cfg),
                crate_name,
            );

            // If `cfg_set` is not `None`, it overrides global cfg settings.
            // Therefore, we have to explicitly add `initial_cfg_set` to `cfg_set` of
            // workspace members to enable test code analysis.
            // For non-workspace members we only add `cfg(target: 'test')` to make sure
            // importing test items tagged with `cfg(test)`
            // from dependencies emits proper diagnostics.
            let cfg_set = if metadata.workspace.members.contains(&component.package) {
                cfg_set_from_scarb.map(|cfg_set| {
                    cfg_set.union(&CfgSet::from_iter([
                        Cfg::name("test"),
                        Cfg::kv("target", "test"),
                    ]))
                })
            } else {
                cfg_set_from_scarb
                    .map(|cfg_set| cfg_set.union(&CfgSet::from_iter([Cfg::kv("target", "test")])))
            }
            .map(|cfg_set| {
                let empty_cfg_set = CfgSet::new();
                let previous_cfg_set = crates_by_component_id
                    .get(&component_id)
                    .and_then(|cr| cr.settings.cfg_set.as_ref())
                    .unwrap_or(&empty_cfg_set);

                cfg_set.union(previous_cfg_set)
            });

            let (regular_dependencies, plugin_dependencies) = component
                .dependencies
                .as_deref()
                .unwrap_or_else(|| {
                    eprintln!(
                        "dependencies of component {crate_name} with id {component_id:?} not \
                         found in metadata",
                    );
                    &[]
                })
                .iter()
                .fold(
                    (Vec::new(), Vec::new()),
                    |(mut regular_deps, mut plugin_deps),
                     CompilationUnitComponentDependencyMetadata { id, .. }| {
                        let regular_dep = compilation_unit
                            .components
                            .iter()
                            .find(|component| component.id.as_ref() == Some(id));

                        let plugin_dep = compilation_unit
                            .cairo_plugins
                            .iter()
                            .find(|plugin| plugin.component_dependency_id.as_ref() == Some(id));

                        match (regular_dep, plugin_dep) {
                            (Some(dep), None) => {
                                regular_deps.push(dep);
                            }
                            (None, Some(dep)) => {
                                plugin_deps.push(dep);
                            }
                            (Some(dep), Some(_)) => {
                                eprintln!("component dependency with id `{}` found in both components and plugins of CU with id `{}`: \
                                        defaulting to treating it as a component dependency", id, compilation_unit.id);
                                regular_deps.push(dep);
                            }
                            (None, None) => {
                                eprintln!("component dependency with id `{}` not found in components nor in plugins of CU with id `{}`", id, compilation_unit.id);
                            }
                        }

                        (regular_deps, plugin_deps)
                    },
                );

            let dependencies = regular_dependencies
                .clone()
                .into_iter()
                .map(|c| {
                    (
                        c.name.clone(),
                        DependencySettings {
                            discriminator: c
                                .discriminator
                                .as_ref()
                                .map(ToSmolStr::to_smolstr)
                                .on_none(|| {
                                    let pkg = metadata
                                        .packages
                                        .iter()
                                        .find(|package| package.id == c.package);

                                    if !is_core(&pkg) {
                                        eprintln!(
                                            "discriminator of component {} with id {} was None",
                                            c.name,
                                            c.id.as_ref().unwrap()
                                        );
                                    }
                                }),
                        },
                    )
                })
                .chain(
                    crates_by_component_id
                        .get(&component_id)
                        .map(|cr| cr.settings.dependencies.clone())
                        .unwrap_or_default(),
                )
                .collect();

            let settings = CrateSettings {
                name: Some(crate_name.into()),
                edition,
                version,
                dependencies,
                cfg_set,
                experimental_features,
            };

            // We collect only the built-in plugins.
            // Procedural macros are handled separately in the
            // `crate::lang::proc_macros::controller`.
            let mut builtin_plugins = crates_by_component_id
                .get(&component_id)
                .map(|cr| cr.builtin_plugins.clone())
                .unwrap_or_default();

            builtin_plugins.extend(if is_core(&Some(package)) {
                // Corelib is a special case because it is described by `cairo_project.toml`.
                plugins_for_corelib()
            } else {
                plugins_from_dependencies(metadata, &plugin_dependencies)
            });

            // It is normally handled with a proc macro server.
            // It is there to prevent annoying diagnostics.
            if regular_dependencies.iter().any(|p| p.name == "snforge_std") {
                builtin_plugins.insert(BuiltinPlugin::SnforgeScarbPlugin);
            }

            let cr = Crate {
                name: crate_name.into(),
                discriminator: component.discriminator.as_ref().map(ToSmolStr::to_smolstr),
                root: root.into(),
                custom_main_file_stems,
                settings,
                builtin_plugins,
            };

            if compilation_unit.package == component.package {
                if let Some(group_id) = compilation_unit.target.params.get("group-id") {
                    if let Some(group_id) = group_id.as_str() {
                        if cr.custom_main_file_stems.is_none() {
                            eprintln!(
                                "compilation unit component with name {crate_name} has `lib.cairo` root \
                                 file while being part of target grouped by group_id {group_id}",
                            )
                        } else {
                            let crates = crates_grouped_by_group_id
                                .entry(group_id.to_string())
                                .or_insert(vec![]);
                            crates.push(cr);

                            continue;
                        }
                    } else {
                        eprintln!(
                            "group-id for target {} was not a string",
                            compilation_unit.target.name
                        )
                    }
                }
            }

            crates_by_component_id.insert(component_id, cr);
        }
    }

    let mut crates: Vec<_> = crates_by_component_id.into_values().collect();

    // Merging crates grouped by group id into single crates.
    for (group_id, crs) in crates_grouped_by_group_id {
        if !crs
            .iter()
            .map(|cr_info| (&cr_info.settings, &cr_info.root))
            .all_equal()
        {
            eprintln!(
                "main crates of targets with group_id {group_id} had different at least one of the \
                 following: settings, roots, manifest paths, package ids"
            )
        }
        let first_crate = &crs[0];

        let builtin_plugins = crs
            .iter()
            .flat_map(|cr| cr.builtin_plugins.clone())
            .collect();
        let custom_main_file_stems = crs
            .iter()
            .flat_map(|cr| cr.custom_main_file_stems.clone().unwrap())
            .collect();

        crates.push(Crate {
            // Name and discriminator don't really matter, so we take the first crate's ones.
            name: first_crate.name.clone(),
            discriminator: first_crate.discriminator.clone(),
            // All crates within a group should have the same settings, root and manifest path.
            root: first_crate.root.clone(),
            settings: first_crate.settings.clone(),

            custom_main_file_stems: Some(custom_main_file_stems),
            builtin_plugins,
        });
    }

    if !crates.iter().any(|cr| cr.name == CORELIB_CRATE_NAME) {
        eprintln!("core crate is missing in scarb metadata, did not initialize it");
    }

    crates
}

/// Perform sanity checks on crate _source path_, and chop it into directory path and file stem.
fn validate_and_chop_source_path<'a>(
    source_path: &'a Path,
    crate_name: &str,
) -> Result<(&'a Path, &'a str)> {
    let metadata = fs::metadata(source_path)
        .with_context(|| format!("io error when accessing source path of: {crate_name}"))?;

    ensure!(
        !metadata.is_dir(),
        "source path of component `{crate_name}` must not be a directory: {source_path}",
        source_path = source_path.display()
    );

    let Some(root) = source_path.parent() else {
        bail!(
            "unexpected fs root as a source path of component `{crate_name}`: {source_path}",
            source_path = source_path.display()
        );
    };

    ensure!(
        root.is_absolute(),
        "source path must be absolute: {source_path}",
        source_path = source_path.display()
    );

    let Some(file_stem) = source_path.file_stem() else {
        bail!(
            "failed to get file stem for component `{crate_name}`: {source_path}",
            source_path = source_path.display()
        );
    };

    let Some(file_stem) = file_stem.to_str() else {
        bail!(
            "file stem is not utf-8: {source_path}",
            source_path = source_path.display()
        );
    };

    Ok((root, file_stem))
}

/// Get the [`Edition`] from [`PackageMetadata`], or assume the default edition.
fn scarb_package_edition(package: &PackageMetadata, crate_name: &str) -> Edition {
    package
        .edition
        .clone()
        .and_then(|e| {
            serde_json::from_value(e.into())
                .with_context(|| format!("failed to parse edition of package: {crate_name}"))
                .inspect_err(|e| eprintln!("{e:?}"))
                .ok()
        })
        .unwrap_or_default()
}

/// Convert a slice of [`scarb_metadata::Cfg`]s to a [`CfgSet`].
///
/// The conversion is done the same way as in Scarb (except no panicking):
/// <https://github.com/software-mansion/scarb/blob/9fe97c8eb8620a1e2103e7f5251c5a9189e75716/scarb/src/ops/metadata.rs#L295-L302>
fn scarb_cfg_set_to_cairo(cfg_set: &[scarb_metadata::Cfg], crate_name: &str) -> Option<CfgSet> {
    serde_json::to_value(cfg_set)
        .and_then(serde_json::from_value)
        .with_context(|| {
            format!(
                "scarb metadata cfg did not convert identically to cairo one for crate: \
                 {crate_name}"
            )
        })
        .inspect_err(|e| eprintln!("{e:?}"))
        .ok()
}

/// Get [`ExperimentalFeaturesConfig`] from [`PackageMetadata`] fields.
fn scarb_package_experimental_features(package: &PackageMetadata) -> ExperimentalFeaturesConfig {
    let contains =
        |feature: &str| -> bool { package.experimental_features.iter().any(|f| f == feature) };

    ExperimentalFeaturesConfig {
        negative_impls: contains("negative_impls"),
        associated_item_constraints: contains("associated_item_constraints"),
        coupons: contains("coupons"),
        user_defined_inline_macros: contains("user_defined_inline_macros"),
    }
}

/// Returns all plugins required by the `core` crate.
fn plugins_for_corelib() -> Vec<BuiltinPlugin> {
    vec![BuiltinPlugin::CairoTest, BuiltinPlugin::Executable]
}

/// Returns all built-in plugins described by `dependencies`.
fn plugins_from_dependencies(
    metadata: &Metadata,
    dependencies: &[&CompilationUnitCairoPluginMetadata],
) -> Vec<BuiltinPlugin> {
    dependencies
        .iter()
        .filter_map(|plugin_metadata| {
            BuiltinPlugin::from_plugin_metadata(metadata, plugin_metadata)
        })
        .collect()
}

fn is_core(package: &Option<&PackageMetadata>) -> bool {
    package.is_some_and(|p| p.name == CORELIB_CRATE_NAME)
}

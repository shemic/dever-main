//! The non-secret deployment contract consumed by the compiler.

use super::{Database, RuntimeProfile, Settings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationBindings {
    pub databases: BTreeMap<String, DatabaseDriver>,
    pub providers: BTreeMap<String, String>,
    pub sites: BTreeMap<String, CompilationSite>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseDriver {
    Sqlite,
    Postgres,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationSite {
    pub path: String,
    pub auth: String,
}

impl Settings {
    pub fn compilation_bindings(&self) -> CompilationBindings {
        CompilationBindings {
            databases: self
                .databases
                .iter()
                .map(|(name, database)| {
                    (
                        name.clone(),
                        match database {
                            Database::Sqlite { .. } => DatabaseDriver::Sqlite,
                            Database::Postgres { .. } => DatabaseDriver::Postgres,
                        },
                    )
                })
                .collect(),
            providers: self
                .auth
                .providers
                .iter()
                .map(|(key, provider)| (key.clone(), provider.verify.clone()))
                .collect(),
            sites: self
                .sites
                .iter()
                .map(|(key, site)| {
                    (
                        key.clone(),
                        CompilationSite {
                            path: site.path.clone(),
                            auth: site.auth.clone(),
                        },
                    )
                })
                .collect(),
        }
    }
}

impl CompilationBindings {
    pub fn validate(&self) -> Result<(), String> {
        if !self.databases.is_empty() && !self.databases.contains_key("default") {
            return Err("database.default is required".into());
        }
        for name in self.databases.keys() {
            validate_database_name(name)?;
        }
        for (key, verify) in &self.providers {
            validate_provider(key, verify)?;
        }
        for (key, site) in &self.sites {
            validate_site(
                key,
                &site.path,
                &site.auth,
                self.providers.contains_key(&site.auth),
            )?;
        }
        validate_site_paths(
            self.sites
                .iter()
                .map(|(key, site)| (key.as_str(), site.path.as_str())),
        )
    }

    pub fn site_for_directory(
        &self,
        directory: &[&str],
    ) -> Result<Option<(&str, &CompilationSite)>, String> {
        select_site(&self.sites, directory, |site| &site.path)
    }

    pub fn resolve_database(
        &self,
        explicit: Option<&str>,
        package_root: &str,
    ) -> Result<(&str, &DatabaseDriver), String> {
        resolve_database(&self.databases, explicit, package_root)
    }

    pub fn profile(&self) -> RuntimeProfile {
        RuntimeProfile {
            sqlite: self
                .databases
                .values()
                .any(|driver| *driver == DatabaseDriver::Sqlite),
            postgres: self
                .databases
                .values()
                .any(|driver| *driver == DatabaseDriver::Postgres),
        }
    }
}

pub(super) fn resolve_database<'a, T>(
    databases: &'a BTreeMap<String, T>,
    explicit: Option<&str>,
    package_root: &str,
) -> Result<(&'a str, &'a T), String> {
    let name = match explicit {
        Some(name) => name,
        None if databases.contains_key(package_root) => package_root,
        None => "default",
    };
    databases
        .get_key_value(name)
        .map(|(name, database)| (name.as_str(), database))
        .ok_or_else(|| format!("database connection '{name}' is not configured"))
}

pub(super) fn select_site<'a, T>(
    sites: &'a BTreeMap<String, T>,
    directory: &[&str],
    path: impl Fn(&T) -> &str,
) -> Result<Option<(&'a str, &'a T)>, String> {
    let mut matches = sites.iter().filter(|(_, site)| {
        let path = path(site);
        path.is_empty()
            || path.split('/').count() <= directory.len()
                && path
                    .split('/')
                    .zip(directory)
                    .all(|(expected, actual)| expected == *actual)
    });
    let selected = matches.next();
    if matches.next().is_some() {
        return Err("API directory matches multiple sites".into());
    }
    Ok(selected.map(|(key, site)| (key.as_str(), site)))
}

pub(super) fn validate_database_name(name: &str) -> Result<(), String> {
    if !super::valid_name(name) {
        return Err(format!(
            "database connection name '{name}' must use lower_snake_case"
        ));
    }
    Ok(())
}

pub(super) fn validate_provider(key: &str, verify: &str) -> Result<(), String> {
    if !super::valid_name(key) {
        return Err(format!("auth provider '{key}' must use lower_snake_case"));
    }
    let parts = verify.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| !super::valid_name(part)) {
        return Err(format!(
            "auth.providers.{key}.verify must be component.domain.function"
        ));
    }
    Ok(())
}

pub(super) fn validate_site(
    key: &str,
    path: &str,
    auth: &str,
    provider_exists: bool,
) -> Result<(), String> {
    if !super::valid_name(key) {
        return Err(format!("site '{key}' must use lower_snake_case"));
    }
    if !path.is_empty() {
        if path.starts_with('/') || path.ends_with('/') {
            return Err(format!(
                "sites.{key}.path must be relative to the api directory"
            ));
        }
        if path.split('/').any(|part| !super::valid_name(part)) {
            return Err(format!(
                "sites.{key}.path segments must use lower_snake_case"
            ));
        }
    }
    if !provider_exists {
        return Err(format!(
            "sites.{key}.auth references unknown provider '{auth}'"
        ));
    }
    Ok(())
}

pub(super) fn validate_site_paths<'a>(
    sites: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<(), String> {
    let mut entries = sites.collect::<Vec<_>>();
    entries.sort_by_key(|(_, path)| *path);
    for adjacent in entries.windows(2) {
        let [(left_key, left), (right_key, right)] = adjacent else {
            unreachable!()
        };
        if left.is_empty()
            || left == right
            || right
                .strip_prefix(*left)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(format!(
                "sites.{left_key}.path and sites.{right_key}.path overlap; site ownership must be unambiguous"
            ));
        }
    }
    Ok(())
}

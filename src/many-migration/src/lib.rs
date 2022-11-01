#![feature(const_mut_refs)]

use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use strum::Display;
use tracing::trace;

pub type FnPtr<T, E> = dyn Sync + Fn(&mut T) -> Result<(), E>;
pub type FnByte = fn(&[u8]) -> Option<Vec<u8>>;

#[derive(Debug, Default, Deserialize, Display, PartialEq, Eq)]
pub enum Status {
    Enabled,
    #[default]
    Disabled,
}

impl Status {
    pub fn enabled() -> Self {
        Status::Enabled
    }

    pub fn disabled() -> Self {
        Status::Disabled
    }
}

#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub block_height: u64,
    pub issue: Option<String>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            block_height: 1,
            issue: None,
            extra: HashMap::default(),
        }
    }
}

#[derive(Clone)]
pub enum MigrationType<'a, T, E> {
    Regular(RegularMigration<'a, T, E>),
    Hotfix(HotfixMigration),
}

// TODO: DRY
impl<'a, T, E> fmt::Display for MigrationType<'a, T, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(match self {
            MigrationType::Regular(_) => "Regular",
            MigrationType::Hotfix(_) => "Hotfix",
        })
    }
}

impl<'a, T, E> fmt::Debug for MigrationType<'a, T, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(match self {
            MigrationType::Regular(_) => "Regular",
            MigrationType::Hotfix(_) => "Hotfix",
        })
    }
}

#[derive(Clone)]
pub struct RegularMigration<'a, T, E> {
    initialize_fn: &'a FnPtr<T, E>,
    update_fn: &'a FnPtr<T, E>,
}

#[derive(Clone)]
pub struct HotfixMigration {
    hotfix_fn: FnByte,
}

#[derive(Clone)]
pub struct InnerMigration<'a, T, E> {
    r#type: MigrationType<'a, T, E>,
    name: &'a str,
    description: &'a str,
}

impl<'a, T, E> fmt::Display for InnerMigration<'a, T, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_fmt(format_args!(
            "Type: \"{}\", Name: \"{}\", Description: \"{}\"",
            self.r#type(),
            self.name(),
            self.description()
        ))
    }
}

impl<'a, T, E> fmt::Debug for InnerMigration<'a, T, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> std::fmt::Result {
        formatter
            .debug_struct("InnerMigration")
            .field("type", &self.r#type)
            .field("name", &self.name)
            .field("description", &self.description)
            .finish()
    }
}

pub struct Migration<'a, T, E> {
    pub migration: &'a InnerMigration<'a, T, E>,
    pub metadata: Metadata,
    pub status: Status,
}

impl<'a, T, E> fmt::Display for Migration<'a, T, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_fmt(format_args!(
            "{}, Metadata: \"{:?}\", Status: \"{}\"",
            self.migration,
            self.metadata(),
            self.status()
        ))
    }
}
impl<'a, T, E> fmt::Debug for Migration<'a, T, E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Migration")
            .field("migration", &self.migration)
            .field("metadata", &self.metadata)
            .field("status", &self.status)
            .finish()
    }
}

impl<'a, T, E> Migration<'a, T, E> {
    pub const fn new(
        migration: &'a InnerMigration<'a, T, E>,
        metadata: Metadata,
        status: Status,
    ) -> Self {
        Self {
            migration,
            metadata,
            status,
        }
    }

    /// This function gets executed when the storage block height == the migration block height
    pub fn initialize(&self, storage: &mut T, h: u64) -> Result<(), E> {
        if self.status == Status::Enabled && self.metadata().block_height == h {
            trace!("Trying to initialize migration - {}", self.name());
            return self.migration.initialize(storage);
        }
        Ok(())
    }

    /// This function gets executed when the storage block height >= the migration block height
    pub fn update(&self, storage: &mut T, h: u64) -> Result<(), E> {
        if self.status == Status::Enabled && self.metadata().block_height >= h {
            trace!("Trying to update migration - {}", self.name());
            return self.migration.update(storage);
        }
        Ok(())
    }

    /// This function gets executed when the storage block height == the migration block height
    pub fn hotfix<'b>(&'b self, b: &'b [u8], h: u64) -> Option<Vec<u8>> {
        if self.status == Status::Enabled && self.metadata().block_height == h {
            trace!("Trying to execute hotfix - {}", self.name());
            return self.migration.hotfix(b);
        }
        None
    }

    pub fn name(&self) -> &'a str {
        self.migration.name()
    }

    pub fn description(&self) -> &'a str {
        self.migration.description()
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn disable(&mut self) {
        self.status = Status::Disabled
    }

    pub fn enable(&mut self) {
        self.status = Status::Enabled
    }

    pub fn is_enabled(&self) -> bool {
        self.status == Status::Enabled
    }
}

impl<'a, T, E> InnerMigration<'a, T, E> {
    pub const fn new_hotfix(hotfix_fn: FnByte, name: &'a str, description: &'a str) -> Self {
        Self {
            r#type: MigrationType::Hotfix(HotfixMigration { hotfix_fn }),
            name,
            description,
        }
    }

    pub const fn new_initialize_update(
        initialize_fn: &'a FnPtr<T, E>,
        update_fn: &'a FnPtr<T, E>,
        name: &'a str,
        description: &'a str,
    ) -> Self {
        Self {
            r#type: MigrationType::Regular(RegularMigration {
                initialize_fn,
                update_fn,
            }),
            name,
            description,
        }
    }

    pub const fn new_initialize(
        initialize_fn: &'a FnPtr<T, E>,
        name: &'a str,
        description: &'a str,
    ) -> Self {
        Self {
            r#type: MigrationType::Regular(RegularMigration {
                initialize_fn,
                update_fn: &|_| Ok(()),
            }),
            name,
            description,
        }
    }

    pub const fn new_update(
        update_fn: &'a FnPtr<T, E>,
        name: &'a str,
        description: &'a str,
    ) -> Self {
        Self {
            r#type: MigrationType::Regular(RegularMigration {
                initialize_fn: &|_| Ok(()),
                update_fn,
            }),
            name,
            description,
        }
    }

    pub const fn name(&self) -> &'a str {
        self.name
    }

    pub const fn description(&self) -> &'a str {
        self.description
    }

    pub const fn r#type(&self) -> &MigrationType<'a, T, E> {
        &self.r#type
    }

    /// This function gets executed when the storage block height == the migration block height
    pub fn initialize(&self, storage: &mut T) -> Result<(), E> {
        match &self.r#type {
            MigrationType::Regular(migration) => (migration.initialize_fn)(storage),
            _ => {
                tracing::trace!(
                    "Migration {} is not of type `Regular`, skipping",
                    self.name()
                );
                Ok(())
            }
        }
    }

    /// This function gets executed when the storage block height >= the migration block height
    pub fn update(&self, storage: &mut T) -> Result<(), E> {
        match &self.r#type {
            MigrationType::Regular(migration) => (migration.update_fn)(storage),
            _ => {
                tracing::trace!(
                    "Migration {} is not of type `Regular`, skipping",
                    self.name()
                );
                Ok(())
            }
        }
    }

    /// This function gets executed when the storage block height == the migration block height
    pub fn hotfix<'b>(&'b self, b: &'b [u8]) -> Option<Vec<u8>> {
        match &self.r#type {
            MigrationType::Hotfix(migration) => (migration.hotfix_fn)(b),
            _ => {
                tracing::trace!(
                    "Migration {} is not of type `Hotfix`, skipping",
                    self.name()
                );
                None
            }
        }
    }
}

#[derive(Deserialize)]
struct IO<'a> {
    r#type: &'a str,

    #[serde(flatten)]
    metadata: Metadata,
}

pub fn load_migrations<'de: 'a, 'a, 'b, E, T>(
    registry: &'b [InnerMigration<'b, T, E>],
    data: &'a str,
) -> Result<BTreeMap<&'b str, Migration<'b, T, E>>, String> {
    // TODO: Do not hardcode the deserializer
    let config: Vec<IO> = serde_json::from_str(data).unwrap();

    // Build a BTreeMap from the linear registry
    let registry = registry
        .iter()
        .map(|m| (m.name, m))
        .collect::<BTreeMap<&'b str, &InnerMigration<'b, T, E>>>();

    Ok(config
        .into_iter()
        .map(|io| {
            let (&k, &v) = registry
                .get_key_value(io.r#type)
                .ok_or_else(|| format!("Unsupported migration type {}", io.r#type))?;
            Ok((k, Migration::new(v, io.metadata, Status::Enabled)))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?
        .into_iter()
        .collect())
}

/// Enable all migrations from the registry EXCEPT the hotfix
pub fn load_enable_all_regular_migrations<'a, E, T>(
    registry: &'a [InnerMigration<'a, T, E>],
) -> BTreeMap<&'a str, Migration<'a, T, E>> {
    registry
        .iter()
        .map(|m| {
            (
                m.name,
                Migration::new(
                    m,
                    Metadata::default(),
                    match m.r#type {
                        MigrationType::Regular(_) => Status::Enabled,
                        MigrationType::Hotfix(_) => Status::Disabled,
                    },
                ),
            )
        })
        .collect()
}

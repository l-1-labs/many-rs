use many_error::ManyError;
use many_identity::Address;
use many_macros::many_module;

pub mod deploy;
pub mod info;
pub mod list;
pub mod remove;

pub use deploy::*;
pub use info::*;
pub use list::*;
pub use remove::*;

#[cfg(test)]
use mockall::{automock, predicate::*};

#[many_module(name = WebModule, id = 16, namespace = web, many_modules_crate = crate)]
#[cfg_attr(test, automock)]
pub trait WebModuleBackend: Send {
    fn info(&self, sender: &Address, args: InfoArg) -> Result<InfoReturns, ManyError>;

    #[many(deny_anonymous)]
    fn deploy(&mut self, sender: &Address, args: DeployArgs) -> Result<DeployReturns, ManyError>;

    #[many(deny_anonymous)]
    fn remove(&mut self, sender: &Address, args: RemoveArgs) -> Result<RemoveReturns, ManyError>;

    fn list(&self, sender: &Address, args: ListArgs) -> Result<ListReturns, ManyError>;
}

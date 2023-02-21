use crate::module::LedgerModuleImpl;
use coset::CoseSign1;
use many_error::{ManyError, ManyErrorCode};
use many_identity::Address;
use many_modules::account::features::{multisig, FeatureId, FeatureInfo, TryCreateFeature};
use many_modules::account::{Account, AccountModuleBackend, Role};
use many_modules::{account, EmptyReturn, ManyModule, ManyModuleInfo};
use many_protocol::{context::Context, RequestMessage, ResponseMessage};
use many_types::cbor::CborAny;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Formatter};

fn get_roles_for_account(account: &account::Account) -> BTreeSet<account::Role> {
    let features = account.features();

    let mut roles = BTreeSet::new();

    // TODO: somehow keep this list updated with the below.
    if features.has_id(multisig::MultisigAccountFeature::ID) {
        roles.append(&mut multisig::MultisigAccountFeature::roles());
    }
    if features.has_id(account::features::ledger::AccountLedger::ID) {
        roles.append(&mut account::features::ledger::AccountLedger::roles());
    }
    if features.has_id(account::features::tokens::TokenAccountLedger::ID) {
        roles.append(&mut account::features::tokens::TokenAccountLedger::roles());
    }

    roles
}

pub(crate) fn validate_features_for_account(account: &account::Account) -> Result<(), ManyError> {
    let features = account.features();

    // TODO: somehow keep this list updated with the above.
    if let Err(e) = features.get::<multisig::MultisigAccountFeature>() {
        if e.code() != ManyErrorCode::AttributeNotFound {
            return Err(e);
        }
    }
    if let Err(e) = features.get::<account::features::ledger::AccountLedger>() {
        if e.code() != ManyErrorCode::AttributeNotFound {
            return Err(e);
        }
    }

    if let Err(e) = features.get::<account::features::tokens::TokenAccountLedger>() {
        if e.code() != ManyErrorCode::AttributeNotFound {
            return Err(e);
        }
    }

    Ok(())
}

pub(crate) fn validate_roles_for_account(account: &account::Account) -> Result<(), ManyError> {
    let features = account.features();

    let mut allowed_roles = BTreeSet::from([account::Role::Owner]);
    let mut account_roles = BTreeSet::<account::Role>::new();
    for (_, r) in account.roles.iter() {
        account_roles.extend(r.iter())
    }

    // TODO: somehow keep this list updated with the above.
    if features.get::<multisig::MultisigAccountFeature>().is_ok() {
        allowed_roles.append(&mut multisig::MultisigAccountFeature::roles());
    }
    if features
        .get::<account::features::ledger::AccountLedger>()
        .is_ok()
    {
        allowed_roles.append(&mut account::features::ledger::AccountLedger::roles());
    }
    if features
        .get::<account::features::tokens::TokenAccountLedger>()
        .is_ok()
    {
        allowed_roles.append(&mut account::features::tokens::TokenAccountLedger::roles());
    }

    for r in account_roles {
        if !allowed_roles.contains(&r) {
            return Err(account::errors::unknown_role(r.to_string()));
        }
    }

    Ok(())
}

pub(crate) fn validate_account(account: &account::Account) -> Result<(), ManyError> {
    // Verify that we support all features.
    validate_features_for_account(account)?;

    // Verify the roles are supported by the features
    validate_roles_for_account(account)?;

    Ok(())
}

pub(crate) fn verify_account_role<R: TryInto<Role> + std::fmt::Display + Copy>(
    account: &Account,
    sender: &Address,
    feature_id: FeatureId,
    role: impl IntoIterator<Item = R>,
) -> Result<(), ManyError> {
    if !account.has_role(sender, account::Role::Owner) {
        if account.features.has_id(feature_id) {
            account.needs_role(sender, role)?;
        } else {
            return Err(super::error::unauthorized());
        }
    }
    Ok(())
}

// TODO: Add proofs for these endpoints.
impl AccountModuleBackend for LedgerModuleImpl {
    fn create(
        &mut self,
        sender: &Address,
        args: account::CreateArgs,
    ) -> Result<account::CreateReturn, ManyError> {
        if args.features.is_empty() {
            return Err(account::errors::empty_feature());
        }
        let account = account::Account::create(sender, args);

        validate_account(&account)?;

        let (id, _) = self.storage.add_account(account)?;
        Ok(account::CreateReturn { id })
    }

    fn set_description(
        &mut self,
        sender: &Address,
        args: account::SetDescriptionArgs,
    ) -> Result<EmptyReturn, ManyError> {
        let (account, _) = self.storage.get_account(&args.account)?;

        if !account.has_role(sender, account::Role::Owner) {
            return Err(account::errors::user_needs_role("owner"));
        }

        self.storage
            .set_description(account, args)
            .map(|_| EmptyReturn)
    }

    fn list_roles(
        &self,
        _: &Address,
        args: account::ListRolesArgs,
        context: Context,
    ) -> Result<account::ListRolesReturn, ManyError> {
        self.storage
            .get_account(&args.account)
            .and_then(|(account, keys)| {
                self.storage
                    .prove_state(context, keys)
                    .map(|_| account::ListRolesReturn {
                        roles: get_roles_for_account(&account),
                    })
            })
    }

    fn get_roles(
        &self,
        _: &Address,
        args: account::GetRolesArgs,
        context: Context,
    ) -> Result<account::GetRolesReturn, ManyError> {
        self.storage
            .get_account(&args.account)
            .and_then(|(account, keys)| {
                self.storage
                    .prove_state(context, keys)
                    .map(|_| account::GetRolesReturn {
                        roles: args
                            .identities
                            .into_iter()
                            .map(|id| (id, account.get_roles(&id)))
                            .collect::<BTreeMap<_, _>>(),
                    })
            })
    }

    fn add_roles(
        &mut self,
        sender: &Address,
        args: account::AddRolesArgs,
    ) -> Result<EmptyReturn, ManyError> {
        let (account, _) = self.storage.get_account(&args.account)?;

        if !account.has_role(sender, account::Role::Owner) {
            Err(account::errors::user_needs_role("owner"))
        } else {
            self.storage.add_roles(account, args).map(|_| EmptyReturn)
        }
    }

    fn remove_roles(
        &mut self,
        sender: &Address,
        args: account::RemoveRolesArgs,
    ) -> Result<EmptyReturn, ManyError> {
        let (account, _) = self.storage.get_account(&args.account)?;

        if !account.has_role(sender, account::Role::Owner) {
            Err(account::errors::user_needs_role(account::Role::Owner))
        } else {
            self.storage
                .remove_roles(account, args)
                .map(|_| EmptyReturn)
        }
    }

    fn info(
        &self,
        _: &Address,
        args: account::InfoArgs,
        context: Context,
    ) -> Result<account::InfoReturn, ManyError> {
        self.storage
            .get_account_even_disabled(&args.account)
            .and_then(
                |(
                    account::Account {
                        description,
                        roles,
                        features,
                        disabled,
                    },
                    keys,
                )| {
                    self.storage
                        .prove_state(context, keys)
                        .map(|_| account::InfoReturn {
                            description,
                            roles,
                            features,
                            disabled,
                        })
                },
            )
    }

    fn disable(
        &mut self,
        sender: &Address,
        args: account::DisableArgs,
    ) -> Result<EmptyReturn, ManyError> {
        let (account, _) = self.storage.get_account(&args.account)?;

        if !account.has_role(sender, account::Role::Owner) {
            Err(account::errors::user_needs_role(account::Role::Owner))
        } else {
            self.storage
                .disable_account(&args.account)
                .map(|_| EmptyReturn)
        }
    }

    fn add_features(
        &mut self,
        sender: &Address,
        args: account::AddFeaturesArgs,
    ) -> Result<account::AddFeaturesReturn, ManyError> {
        if args.features.is_empty() {
            Err(account::errors::empty_feature())
        } else {
            let (account, _) = self.storage.get_account(&args.account)?;

            account.needs_role(sender, [account::Role::Owner])?;
            self.storage
                .add_features(account, args)
                .map(|_| EmptyReturn)
        }
    }
}

/// A module for returning the features by this account.
pub struct AccountFeatureModule<T: AccountModuleBackend> {
    inner: account::AccountModule<T>,
    info: ManyModuleInfo,
}

impl<T: AccountModuleBackend> AccountFeatureModule<T> {
    pub fn new(
        inner: account::AccountModule<T>,
        features: impl IntoIterator<Item = account::features::Feature>,
    ) -> Self {
        let mut info: ManyModuleInfo = inner.info().clone();
        info.attribute = info.attribute.map(|mut a| {
            for f in features.into_iter() {
                a.arguments.push(CborAny::Int(f.id() as i64));
            }
            a
        });

        Self { inner, info }
    }
}

impl<T: AccountModuleBackend> Debug for AccountFeatureModule<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccountFeatureModule")
    }
}

#[async_trait::async_trait]
impl<T: AccountModuleBackend> ManyModule for AccountFeatureModule<T> {
    fn info(&self) -> &ManyModuleInfo {
        &self.info
    }

    fn validate(&self, message: &RequestMessage, envelope: &CoseSign1) -> Result<(), ManyError> {
        self.inner.validate(message, envelope)
    }

    async fn execute(&self, message: RequestMessage) -> Result<ResponseMessage, ManyError> {
        self.inner.execute(message).await
    }
}

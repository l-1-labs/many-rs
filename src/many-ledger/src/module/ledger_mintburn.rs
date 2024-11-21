use crate::error;
use crate::migration::disable_token_mint::DISABLE_TOKEN_MINT_MIGRATION;
use crate::migration::tokens::TOKEN_MIGRATION;
use crate::module::LedgerModuleImpl;
use crate::storage::ledger_tokens::verify_tokens_sender;
use many_error::ManyError;
use many_identity::Address;
use many_modules::events::EventInfo;
use many_modules::ledger;
use many_modules::ledger::{TokenBurnArgs, TokenBurnReturns, TokenMintArgs, TokenMintReturns};
use many_types::ledger::Symbol;
use once_cell::sync::Lazy;
use std::collections::BTreeSet;
use std::str::FromStr;

// Production network MFX address
pub static MFX: Lazy<Address> = Lazy::new(|| {
    Address::from_str("mqbh742x4s356ddaryrxaowt4wxtlocekzpufodvowrirfrqaaaaa3l").unwrap()
});

/// Check if a symbol exists in the storage
fn check_symbol_exists(symbol: &Symbol, symbols: BTreeSet<Symbol>) -> Result<(), ManyError> {
    if !symbols.contains(symbol) {
        Err(error::symbol_not_found(symbol.to_string()))
    } else {
        Ok(())
    }
}

impl ledger::LedgerMintBurnModuleBackend for LedgerModuleImpl {
    fn mint(
        &mut self,
        sender: &Address,
        args: TokenMintArgs,
    ) -> Result<TokenMintReturns, ManyError> {
        if !self.storage.migrations().is_active(&TOKEN_MIGRATION) {
            return Err(ManyError::invalid_method_name("tokens.mint"));
        }

        let TokenMintArgs {
            symbol,
            distribution,
            memo,
        } = args;

        if symbol != *MFX
            && self
                .storage
                .migrations()
                .is_active(&DISABLE_TOKEN_MINT_MIGRATION)
        {
            return Err(ManyError::unknown(
                "Token minting is disabled on this network",
            ));
        }

        self.verify_mint_burn_identity(sender, &symbol)?;

        check_symbol_exists(&symbol, self.storage.get_symbols()?)?;

        // Mint into storage
        let _ = self.storage.mint_token(symbol, &distribution)?;

        // Log event
        self.storage
            .log_event(EventInfo::TokenMint {
                symbol,
                distribution,
                memo,
            })
            .map(|_| TokenMintReturns {})
    }

    fn burn(
        &mut self,
        sender: &Address,
        args: TokenBurnArgs,
    ) -> Result<TokenBurnReturns, ManyError> {
        if !self.storage.migrations().is_active(&TOKEN_MIGRATION) {
            return Err(ManyError::invalid_method_name("tokens.burn"));
        }

        let TokenBurnArgs {
            symbol,
            distribution,
            memo,
            error_on_under_burn,
        } = args;

        self.verify_mint_burn_identity(sender, &symbol)?;

        check_symbol_exists(&symbol, self.storage.get_symbols()?)?;

        // Disable partial burn, for now
        if let Some(error) = error_on_under_burn {
            if !error {
                return Err(error::partial_burn_disabled());
            }
        }

        // Burn from storage
        let _ = self.storage.burn_token(symbol, &distribution)?;

        // Log event
        self.storage
            .log_event(EventInfo::TokenBurn {
                symbol,
                distribution: distribution.clone(),
                memo,
            })
            .map(|_| TokenBurnReturns { distribution })
    }
}

impl LedgerModuleImpl {
    /// Only the token identity, the server identity or the token owner is allowed to mint/burn
    fn verify_mint_burn_identity(
        &mut self,
        sender: &Address,
        symbol: &Symbol,
    ) -> Result<(), ManyError> {
        // Are we the token identity or the server identity?
        verify_tokens_sender(
            sender,
            self.storage
                .get_identity(crate::storage::ledger_tokens::TOKEN_IDENTITY_ROOT)
                .or_else(|_| self.storage.get_identity(crate::storage::IDENTITY_ROOT))?,
        )
        // Are we the token owner?
        .or_else(|_| match self.storage.get_owner(symbol) {
            Ok((Some(token_owner), _)) => verify_tokens_sender(sender, token_owner),
            _ => Err(error::no_token_owner()),
        })?;
        Ok(())
    }
}

use crate::error;
use crate::storage::{key_for_account_balance, LedgerStorage, IDENTITY_ROOT, SYMBOLS_ROOT};
use many_error::ManyError;
use many_identity::Address;
use many_protocol::context::Context;
use many_types::{
    ledger::{Symbol, TokenAmount},
    ProofOperation,
};
use merk::{
    proofs::{
        query::QueryItem,
        Decoder,
        Node::{Hash, KVHash, KV},
        Op::{Child, Parent, Push},
    },
    BatchEntry, Op,
};
use std::collections::{BTreeMap, BTreeSet};

impl LedgerStorage {
    pub fn with_balances(
        mut self,
        identity: &Address,
        symbols: &BTreeMap<Symbol, String>,
        initial_balances: &BTreeMap<Address, BTreeMap<Symbol, TokenAmount>>,
    ) -> Result<Self, ManyError> {
        let mut batch: Vec<BatchEntry> = Vec::new();
        for (k, v) in initial_balances.iter() {
            for (symbol, tokens) in v.iter() {
                if !symbols.contains_key(symbol) {
                    return Err(ManyError::unknown(format!(
                        r#"Unknown symbol "{symbol}" for identity {k}"#
                    ))); // TODO: Custom error
                }

                let key = key_for_account_balance(k, symbol);
                batch.push((key, Op::Put(tokens.to_vec())));
            }
        }

        batch.push((
            IDENTITY_ROOT.as_bytes().to_vec(),
            Op::Put(identity.to_vec()),
        ));
        batch.push((
            SYMBOLS_ROOT.as_bytes().to_vec(),
            Op::Put(minicbor::to_vec(symbols).map_err(ManyError::serialization_error)?),
        ));

        batch.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));

        self.persistent_store
            .apply(batch.as_slice())
            .map_err(error::storage_apply_failed)?;

        Ok(self)
    }

    fn get_all_balances(
        &self,
        identity: &Address,
    ) -> Result<
        (
            BTreeMap<Symbol, TokenAmount>,
            impl IntoIterator<Item = Vec<u8>>,
        ),
        ManyError,
    > {
        Ok(if identity.is_anonymous() {
            // Anonymous cannot hold funds.
            (BTreeMap::new(), vec![])
        } else {
            let mut result = BTreeMap::new();
            for symbol in self.get_symbols()? {
                self.persistent_store
                    .get(&key_for_account_balance(identity, &symbol))
                    .map_err(error::storage_get_failed)?
                    .map(|value| result.insert(symbol, TokenAmount::from(value)))
                    .map(|_| ())
                    .unwrap_or_default()
            }

            (
                result,
                self.get_symbols()?
                    .into_iter()
                    .map(|symbol| key_for_account_balance(identity, &symbol))
                    .collect(),
            )
        })
    }

    pub fn get_multiple_balances(
        &self,
        identity: &Address,
        symbols: &BTreeSet<Symbol>,
    ) -> Result<
        (
            BTreeMap<Symbol, TokenAmount>,
            impl IntoIterator<Item = Vec<u8>>,
        ),
        ManyError,
    > {
        self.get_all_balances(identity).map(|(balances, keys)| {
            (
                if symbols.is_empty() {
                    balances
                } else {
                    balances
                        .into_iter()
                        .filter(|(k, _v)| symbols.contains(k))
                        .collect()
                },
                keys,
            )
        })
    }

    pub fn prove_state(
        &self,
        context: impl AsRef<Context>,
        keys: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<(), ManyError> {
        context.as_ref().prove(|| {
            self.persistent_store
                .prove(
                    keys.into_iter()
                        .map(QueryItem::Key)
                        .collect::<Vec<_>>()
                        .into(),
                )
                .and_then(|proof| {
                    Decoder::new(proof.as_slice())
                        .map(|fallible_operation| {
                            fallible_operation.map(|operation| match operation {
                                Child => ProofOperation::Child,
                                Parent => ProofOperation::Parent,
                                Push(Hash(hash)) => ProofOperation::NodeHash(hash.to_vec()),
                                Push(KV(key, value)) => {
                                    ProofOperation::KeyValuePair(key.into(), value.into())
                                }
                                Push(KVHash(hash)) => ProofOperation::KeyValueHash(hash.to_vec()),
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .map_err(|error| ManyError::unknown(error.to_string()))
        })
    }
}

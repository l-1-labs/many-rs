use crate::error;
use crate::storage::LedgerStorage;
use base64::{engine::general_purpose, Engine as _};
use many_error::ManyError;
use many_identity::Address;
use many_modules::idstore;
use merk::Op;
use std::collections::BTreeMap;

pub(crate) const IDSTORE_ROOT: &[u8] = b"/idstore/";
pub(crate) const IDSTORE_SEED_ROOT: &[u8] = b"/config/idstore_seed";

#[derive(Clone, minicbor::Encode, minicbor::Decode)]
#[cbor(map)]
struct CredentialStorage {
    #[n(0)]
    cred_id: idstore::CredentialId,

    #[n(1)]
    public_key: idstore::PublicKey,
}

enum IdStoreRootSeparator {
    RecallPhrase,
    Address,
}

impl IdStoreRootSeparator {
    fn value(&self) -> &[u8] {
        match *self {
            IdStoreRootSeparator::RecallPhrase => b"00",
            IdStoreRootSeparator::Address => b"01",
        }
    }
}

impl LedgerStorage {
    pub fn with_idstore(
        mut self,
        maybe_seed: Option<u64>,
        maybe_keys: Option<BTreeMap<String, String>>,
    ) -> Result<Self, ManyError> {
        let maybe_keys = maybe_keys.map(|keys| {
            keys.iter()
                .map(|(k, v)| {
                    let k = general_purpose::STANDARD
                        .decode(k)
                        .expect("Invalid base64 for key");
                    let v = general_purpose::STANDARD
                        .decode(v)
                        .expect("Invalid base64 for value");
                    (k, v)
                })
                .collect::<BTreeMap<_, _>>()
        });

        // Apply keys and seed.
        if let Some(seed) = maybe_seed {
            self.persistent_store
                .apply(&[(
                    IDSTORE_SEED_ROOT.to_vec(),
                    Op::Put(seed.to_be_bytes().to_vec()),
                )])
                .map_err(error::storage_apply_failed)?;
        }
        if let Some(keys) = maybe_keys {
            for (k, v) in keys {
                self.persistent_store
                    .apply(&[(k, Op::Put(v))])
                    .map_err(error::storage_apply_failed)?;
            }
        }

        Ok(self)
    }

    pub(crate) fn inc_idstore_seed(&mut self) -> Result<u64, ManyError> {
        let idstore_seed = self
            .persistent_store
            .get(IDSTORE_SEED_ROOT)
            .map_err(error::storage_get_failed)?
            .map_or(0u64, |x| {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(x.as_slice());
                u64::from_be_bytes(bytes)
            });

        self.persistent_store
            .apply(&[(
                IDSTORE_SEED_ROOT.to_vec(),
                Op::Put((idstore_seed + 1).to_be_bytes().to_vec()),
            )])
            .map_err(error::storage_apply_failed)?;

        self.maybe_commit().map(|_| idstore_seed)
    }

    pub fn store(
        &mut self,
        recall_phrase: &idstore::RecallPhrase,
        address: &Address,
        cred_id: idstore::CredentialId,
        public_key: idstore::PublicKey,
    ) -> Result<Vec<Vec<u8>>, ManyError> {
        let recall_phrase_cbor =
            minicbor::to_vec(recall_phrase).map_err(ManyError::serialization_error)?;
        if self
            .persistent_store
            .get(&recall_phrase_cbor)
            .map_err(error::storage_get_failed)?
            .is_some()
        {
            return Err(idstore::existing_entry());
        }

        let value = minicbor::to_vec(CredentialStorage {
            cred_id,
            public_key,
        })
        .map_err(ManyError::serialization_error)?;

        let batch = vec![
            (
                [
                    IDSTORE_ROOT,
                    IdStoreRootSeparator::RecallPhrase.value(),
                    &recall_phrase_cbor,
                ]
                .concat(),
                Op::Put(value.clone()),
            ),
            (
                [
                    IDSTORE_ROOT,
                    IdStoreRootSeparator::Address.value(),
                    &address.to_vec(),
                ]
                .concat(),
                Op::Put(value),
            ),
        ];

        self.persistent_store
            .apply(&batch)
            .map_err(error::storage_apply_failed)?;

        self.maybe_commit().map(|_| {
            vec![
                recall_phrase_cbor.clone(),
                [
                    IDSTORE_ROOT,
                    IdStoreRootSeparator::RecallPhrase.value(),
                    &recall_phrase_cbor,
                ]
                .concat(),
                [
                    IDSTORE_ROOT,
                    IdStoreRootSeparator::Address.value(),
                    &address.to_vec(),
                ]
                .concat(),
            ]
        })
    }

    fn get_from_storage(
        &self,
        key: &Vec<u8>,
        sep: IdStoreRootSeparator,
    ) -> Result<(Option<Vec<u8>>, Vec<u8>), ManyError> {
        let key = [IDSTORE_ROOT, sep.value(), key].concat();
        self.persistent_store
            .get(&key)
            .map_err(error::storage_get_failed)
            .map(|value| (value, key))
    }

    pub fn get_from_recall_phrase(
        &self,
        recall_phrase: &idstore::RecallPhrase,
    ) -> Result<(idstore::CredentialId, idstore::PublicKey, Vec<u8>), ManyError> {
        let recall_phrase_cbor =
            minicbor::to_vec(recall_phrase).map_err(ManyError::serialization_error)?;
        if let (Some(value), storage_key) =
            self.get_from_storage(&recall_phrase_cbor, IdStoreRootSeparator::RecallPhrase)?
        {
            let value: CredentialStorage =
                minicbor::decode(&value).map_err(ManyError::deserialization_error)?;
            Ok((value.cred_id, value.public_key, storage_key))
        } else {
            Err(idstore::entry_not_found(recall_phrase.join(" ")))
        }
    }

    pub fn get_from_address(
        &self,
        address: &Address,
    ) -> Result<(idstore::CredentialId, idstore::PublicKey, Vec<u8>), ManyError> {
        if let (Some(value), storage_key) =
            self.get_from_storage(&address.to_vec(), IdStoreRootSeparator::Address)?
        {
            let value: CredentialStorage =
                minicbor::decode(&value).map_err(ManyError::deserialization_error)?;
            Ok((value.cred_id, value.public_key, storage_key))
        } else {
            Err(idstore::entry_not_found(address.to_string()))
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    impl LedgerStorage {
        pub fn set_idstore_seed(&mut self, seed: u64) -> Result<(), ManyError> {
            self.persistent_store
                .apply(&[(
                    IDSTORE_SEED_ROOT.to_vec(),
                    Op::Put(seed.to_be_bytes().to_vec()),
                )])
                .map_err(error::storage_apply_failed)?;

            self.persistent_store
                .commit(&[])
                .map_err(error::storage_commit_failed)?;
            Ok(())
        }
    }
}

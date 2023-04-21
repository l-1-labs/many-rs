use {
    crate::storage::{event::HEIGHT_EVENTID_SHIFT, LedgerStorage},
    many_error::ManyError,
    many_modules::abci_backend::AbciCommitInfo,
    many_modules::events::EventId,
    minicbor::bytes::ByteVec,
};

impl LedgerStorage {
    #[allow(clippy::redundant_closure_call)]
    pub fn commit(&mut self) -> AbciCommitInfo {
        let (retain_height, hash) = (|| -> Result<(u64, ByteVec), ManyError> {
            // First check if there's any need to clean up multisig transactions. Ignore
            // errors.
            let _ = self.check_timed_out_multisig_transactions();

            let height = self.inc_height()?;
            let retain_height = 0;

            // Committing before the migration so that the migration has
            // the actual state of the database when setting its
            // attributes.
            self.commit_storage()?;

            // Initialize/update migrations at current height, if any
            self.migrations.update_at_height(
                &mut self.persistent_store,
                height + 1,
                self.path.clone(),
            )?;

            self.commit_storage()?;

            let hash = self.persistent_store.root_hash().to_vec();
            self.current_hash = Some(hash.clone());

            self.latest_tid = EventId::from(height << HEIGHT_EVENTID_SHIFT);
            Ok((retain_height, hash.into()))
        })()
        .unwrap_or_else(|error| {
            println!("AbciCommitInfo erorr: {error:?}");
            (0, error.to_string().into_bytes().into())
        });

        // TODO: This function's implementation proves that the return type of
        // LedgerModuleImpl's trait method should be Result<AbciCommitInfo, ManyError>
        AbciCommitInfo {
            retain_height,
            hash,
        }
    }
}

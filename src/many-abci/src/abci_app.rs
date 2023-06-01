use crate::migration::error_code::LEGACY_ERROR_CODE_TRIGGER;
use crate::migration::{AbciAppMigrations, MIGRATIONS};
use coset::{CborSerializable, CoseSign1};
use many_client::client::blocking::{block_on, ManyClient};
use many_error::{ManyError, ManyErrorCode};
use many_identity::{Address, AnonymousIdentity};
use many_migration::MigrationConfig;
use many_modules::abci_backend::{AbciBlock, AbciCommitInfo, AbciInfo};
use many_protocol::{RequestMessage, ResponseMessage};
use many_server::RequestValidator;
use reqwest::{IntoUrl, Url};
use std::sync::{Arc, RwLock};
use tendermint_abci::Application;
use tendermint_proto::abci::*;
use tracing::{debug, error};

lazy_static::lazy_static!(
    static ref EPOCH: many_types::Timestamp = many_types::Timestamp::new(0).unwrap();
);

enum ManyAbciErrorCodes {
    Success = 0,
    // The message was not successfully sent to the backend.
    TransportError = 1,
    // An error happened in the ABCI layer itself (serialization, etc).
    FrontendError = 2,
}

enum ManyAbciCheckErrorCodes {
    Success = 0,
    CoseDeserializeError = 4,
    MessageDeserializeError = 5,
    RwLockPoisonedError = 6,
    TimestampError = 7,
    CannotGetSystemTimeError = 8,
    TimestampOutsideOfRangeError = 9,
    ValidationError = 10,
}

enum ManyAbciDeliverErrorCodes {
    Success = 0,

    TransportRequestError = 1,
    CoseDeserializeError = 2,
    TransportResponseError = 3,
    RwLockPoisonedError = 11,
}

pub const MANYABCI_DEFAULT_TIMEOUT: u64 = 300;

fn get_abci_info_(client: &ManyClient<AnonymousIdentity>) -> Result<AbciInfo, ManyError> {
    client
        .call_("abci.info", ())
        .and_then(|payload| minicbor::decode(&payload).map_err(ManyError::deserialization_error))
}

#[derive(Clone)]
pub struct AbciApp {
    app_name: String,
    many_client: ManyClient<AnonymousIdentity>,
    many_url: Url,
    cache: Arc<RwLock<dyn RequestValidator + Send + Sync>>,

    /// We need interior mutability, safely.
    migrations: Arc<RwLock<AbciAppMigrations>>,
    block_time: Arc<RwLock<Option<u64>>>,
}

impl AbciApp {
    /// Constructor.
    pub fn create<U>(
        many_url: U,
        server_id: Address,
        migration_config: Option<MigrationConfig>,
    ) -> Result<Self, String>
    where
        U: IntoUrl,
    {
        let many_url = many_url.into_url().map_err(|e| e.to_string())?;

        // TODO: Get the server ID from the many server.
        // let server_id = if server_id.is_anonymous() {
        //     server_id
        // } else {
        //     server_id
        // };

        let many_client = ManyClient::new(many_url.clone(), server_id, AnonymousIdentity)?;
        let status = many_client.status().map_err(|x| x.to_string())?;
        let app_name = status.name;

        let migrations = RwLock::new({
            let AbciInfo { height, .. } = get_abci_info_(&many_client)
                .map_err(|e| format!("Unable to call abci.info: {e}"))?;

            let migrations = migration_config
                .map_or_else(AbciAppMigrations::empty, |config| {
                    AbciAppMigrations::load(&MIGRATIONS, config, height)
                })
                .map_err(|e| format!("Unable to load migrations: {e}"))?;
            debug!("Final migrations: {:?}", migrations);
            migrations
        });

        Ok(Self {
            app_name,
            many_url,
            many_client,
            cache: Arc::new(RwLock::new(())),
            migrations: Arc::new(migrations),
            block_time: Arc::new(RwLock::new(None)),
        })
    }

    pub fn with_validator<C: RequestValidator + Send + Sync + 'static>(mut self, cache: C) -> Self {
        self.cache = Arc::new(RwLock::new(cache));
        self
    }

    fn do_check_tx(&self, tx: impl AsRef<[u8]>) -> Result<(), (ManyAbciCheckErrorCodes, String)> {
        use many_types::Timestamp;
        let cose = CoseSign1::from_slice(tx.as_ref()).map_err(|log| {
            (
                ManyAbciCheckErrorCodes::CoseDeserializeError,
                log.to_string(),
            )
        })?;
        let message = RequestMessage::try_from(&cose).map_err(|log| {
            (
                ManyAbciCheckErrorCodes::MessageDeserializeError,
                log.to_string(),
            )
        })?;

        // Run the same validator as the server would.
        {
            let validator = self.cache.read().map_err(|log| {
                (
                    ManyAbciCheckErrorCodes::RwLockPoisonedError,
                    log.to_string(),
                )
            })?;
            // Validate the envelope.
            if validator.validate_envelope(&cose).is_err() {
                return Err((
                    ManyAbciCheckErrorCodes::ValidationError,
                    "Transaction already in cache".to_string(),
                ));
            }

            // Validate the message.
            validator
                .validate_request(&message)
                .map_err(|log| (ManyAbciCheckErrorCodes::ValidationError, log.to_string()))?;
        }

        // Check the time of the transaction.
        let time = self.block_time.read().map_err(|log| {
            (
                ManyAbciCheckErrorCodes::RwLockPoisonedError,
                log.to_string(),
            )
        })?;
        let now = time
            .as_ref()
            .map_or_else(|| Ok(Timestamp::now()), |x| Timestamp::new(*x))
            .map_err(|e| (ManyAbciCheckErrorCodes::TimestampError, e.to_string()))?;

        let now = now.as_system_time().map_err(|log| {
            (
                ManyAbciCheckErrorCodes::CannotGetSystemTimeError,
                log.to_string(),
            )
        })?;

        message
            .validate_time(now, MANYABCI_DEFAULT_TIMEOUT)
            .map_err(|log| {
                (
                    ManyAbciCheckErrorCodes::TimestampOutsideOfRangeError,
                    log.to_string(),
                )
            })?;
        Ok(())
    }
}

impl Application for AbciApp {
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        debug!(
            "Got info request. Tendermint version: {}; Block version: {}; P2P version: {}",
            request.version, request.block_version, request.p2p_version
        );

        let AbciInfo { height, hash } = match get_abci_info_(&self.many_client) {
            Ok(x) => x,
            Err(err) => {
                return ResponseInfo {
                    data: format!("An error occurred during call to abci.info:\n{err}"),
                    ..Default::default()
                }
            }
        };

        ResponseInfo {
            data: format!("many-abci-bridge({})", self.app_name),
            version: env!("CARGO_PKG_VERSION").to_string(),
            app_version: 1,
            last_block_height: height as i64,
            last_block_app_hash: hash.to_vec().into(),
        }
    }
    fn init_chain(&self, _request: RequestInitChain) -> ResponseInitChain {
        Default::default()
    }
    fn query(&self, request: RequestQuery) -> ResponseQuery {
        let cose = match CoseSign1::from_slice(&request.data) {
            Ok(x) => x,
            Err(err) => {
                return ResponseQuery {
                    code: ManyAbciErrorCodes::FrontendError as u32,
                    log: err.to_string(),
                    ..Default::default()
                }
            }
        };
        let value = match block_on(many_client::client::send_envelope(
            self.many_url.clone(),
            cose,
        )) {
            Ok(cose_sign) => cose_sign,

            Err(err) => {
                return ResponseQuery {
                    code: ManyAbciErrorCodes::TransportError as u32,
                    log: err.to_string(),
                    ..Default::default()
                }
            }
        };

        match value.to_vec() {
            Ok(value) => ResponseQuery {
                code: ManyAbciErrorCodes::Success as u32,
                value: value.into(),
                ..Default::default()
            },
            Err(err) => ResponseQuery {
                code: ManyAbciErrorCodes::FrontendError as u32,
                log: err.to_string(),
                ..Default::default()
            },
        }
    }

    fn begin_block(&self, request: RequestBeginBlock) -> ResponseBeginBlock {
        let (time, height) = request
            .header
            .map(|x| {
                let time = x.time.map(|x| x.seconds as u64);
                let height = Some(if x.height > 0 { x.height as u64 } else { 0 });

                (time, height)
            })
            .unwrap_or((None, None));

        if let Some(height) = height {
            if let Ok(mut m) = self.migrations.write() {
                // Since it's impossible to truly handle error here, and
                // we don't actually want to panic, just ignore any errors.
                let _ = m.update_at_height(&mut (), height);
            } else {
                error!("Migration: Could not acquire migration lock...");
            }
        }

        let block = AbciBlock { time };
        self.block_time
            .write()
            .map(|mut block_time| *block_time = time)
            .unwrap_or_else(|_| error!("Block time: Could not acquire lock"));
        let _ = self.many_client.call_("abci.beginBlock", block);
        ResponseBeginBlock { events: vec![] }
    }

    fn check_tx(&self, request: RequestCheckTx) -> ResponseCheckTx {
        self.do_check_tx(&request.tx)
            .map(|_| ResponseCheckTx {
                code: ManyAbciCheckErrorCodes::Success as u32,
                ..Default::default()
            })
            .unwrap_or_else(|(code, log)| {
                debug!("check_tx failed: {}", log);
                ResponseCheckTx {
                    code: code as u32,
                    log,
                    ..Default::default()
                }
            })
    }

    fn deliver_tx(&self, request: RequestDeliverTx) -> ResponseDeliverTx {
        let cose = match CoseSign1::from_slice(&request.tx) {
            Ok(x) => x,
            Err(err) => {
                return ResponseDeliverTx {
                    code: ManyAbciDeliverErrorCodes::CoseDeserializeError as u32,
                    log: err.to_string(),
                    ..Default::default()
                }
            }
        };
        match block_on(many_client::client::send_envelope(
            self.many_url.clone(),
            cose.clone(),
        )) {
            Ok(cose_sign) => {
                let payload = cose_sign.payload.unwrap_or_default();
                let mut response = ResponseMessage::from_bytes(&payload).unwrap_or_default();

                // Consensus will sign the result, so the `from` field is unnecessary.
                response.from = Address::anonymous();
                // The version is ignored and removed.
                response.version = None;
                // The timestamp MIGHT differ between two nodes so we just force it to be 0.
                response.timestamp = Some(*EPOCH);

                // Check whether we need to apply a correction to the error code decoding
                // logic.
                // A bug in the Error module was fixed in
                //     https://github.com/liftedinit/many-rs/pull/177
                // which meant we started decoding errors properly, but in production
                // the ledger was genesis before.
                if let Ok(m) = self.migrations.read() {
                    if m.is_active(&LEGACY_ERROR_CODE_TRIGGER) {
                        response.data = match response.data {
                            Err(err) => {
                                if err.code().is_attribute_specific() {
                                    Err(err.with_code(ManyErrorCode::Unknown))
                                } else {
                                    Err(err)
                                }
                            }
                            x => x,
                        };
                    }
                }

                {
                    let cache = self.cache.write();
                    if cache.is_err() {
                        return ResponseDeliverTx {
                            code: ManyAbciDeliverErrorCodes::RwLockPoisonedError as u32,
                            ..Default::default()
                        };
                    }
                    if let Err(e) = cache.unwrap().message_executed(&cose, &response) {
                        // There's nothing we can do here, since the backend has
                        // already executed the message and updated its test.
                        panic!(
                            "message_executed failed: {e}\n\
                            The backend and tendermint states might be inconsistent \
                            and would need to revert to a previous block."
                        );
                    }
                }

                if let Ok(data) = response.to_bytes() {
                    ResponseDeliverTx {
                        code: ManyAbciDeliverErrorCodes::Success as u32,
                        data: data.into(),
                        ..Default::default()
                    }
                } else {
                    ResponseDeliverTx {
                        code: ManyAbciDeliverErrorCodes::TransportResponseError as u32,
                        ..Default::default()
                    }
                }
            }
            Err(err) => ResponseDeliverTx {
                code: ManyAbciDeliverErrorCodes::TransportRequestError as u32,
                log: err.to_string(),
                ..Default::default()
            },
        }
    }

    fn end_block(&self, _request: RequestEndBlock) -> ResponseEndBlock {
        let _ = self.many_client.call_("abci.endBlock", ());
        Default::default()
    }

    fn flush(&self) -> ResponseFlush {
        Default::default()
    }

    fn commit(&self) -> ResponseCommit {
        self.many_client.call_("abci.commit", ()).map_or_else(
            |err| ResponseCommit {
                data: err.to_string().into_bytes().into(),
                retain_height: 0,
            },
            |msg| {
                let info: AbciCommitInfo = minicbor::decode(&msg).unwrap();
                ResponseCommit {
                    data: info.hash.to_vec().into(),
                    retain_height: info.retain_height as i64,
                }
            },
        )
    }
}

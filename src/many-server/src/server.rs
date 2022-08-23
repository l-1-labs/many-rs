use crate::transport::LowLevelManyRequestHandler;
use async_trait::async_trait;
use coset::CoseSign1;
use many_error::ManyError;
use many_identity::CoseKeyIdentity;
use many_modules::{base, ManyModule, ManyModuleInfo};
use many_protocol::{ManyUrl, RequestMessage, ResponseMessage};
use many_types::attributes::Attribute;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Validate that the timestamp of a message is within a timeout, either in the future
/// or the past.
fn _validate_time(
    message: &RequestMessage,
    now: SystemTime,
    timeout_in_secs: u64,
) -> Result<(), ManyError> {
    if timeout_in_secs == 0 {
        return Err(ManyError::timestamp_out_of_range());
    }
    let ts = message
        .timestamp
        .ok_or_else(|| ManyError::required_field_missing("timestamp".to_string()))?
        .as_system_time()?;

    // Get the absolute time difference.
    let (early, later) = if ts < now { (ts, now) } else { (now, ts) };
    let diff = later
        .duration_since(early)
        .map_err(|_| ManyError::timestamp_out_of_range())?;

    if diff.as_secs() >= timeout_in_secs {
        tracing::error!(
            "ERR: Timestamp outside of timeout: {} >= {}",
            diff.as_secs(),
            timeout_in_secs
        );
        return Err(ManyError::timestamp_out_of_range());
    }

    Ok(())
}

trait ManyServerFallback: LowLevelManyRequestHandler + base::BaseModuleBackend {}

impl<M: LowLevelManyRequestHandler + base::BaseModuleBackend + 'static> ManyServerFallback for M {}

#[derive(Debug, Clone)]
pub struct ManyModuleList {}

pub const MANYSERVER_DEFAULT_TIMEOUT: u64 = 300;

#[derive(Default)]
pub struct ManyServer {
    modules: Vec<Arc<dyn ManyModule + Send>>,
    method_cache: BTreeSet<String>,
    identity: CoseKeyIdentity,
    name: String,
    version: Option<String>,
    timeout: u64,
    fallback: Option<Arc<dyn ManyServerFallback + Send + 'static>>,
    allowed_origins: Option<Vec<ManyUrl>>,

    time_fn: Option<Arc<dyn Fn() -> Result<SystemTime, ManyError> + Send + Sync>>,
}

impl ManyServer {
    pub fn simple<N: ToString>(
        name: N,
        identity: CoseKeyIdentity,
        version: Option<String>,
        allow: Option<Vec<ManyUrl>>,
    ) -> Arc<Mutex<Self>> {
        let s = Self::new(name, identity, allow);
        {
            let mut s2 = s.lock().unwrap();
            s2.version = version;
            s2.add_module(base::BaseModule::new(s.clone()));
        }

        s
    }

    pub fn new<N: ToString>(
        name: N,
        identity: CoseKeyIdentity,
        allowed_origins: Option<Vec<ManyUrl>>,
    ) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            name: name.to_string(),
            identity,
            timeout: MANYSERVER_DEFAULT_TIMEOUT,
            allowed_origins,
            ..Default::default()
        }))
    }

    pub fn set_timeout(&mut self, timeout_in_secs: u64) {
        self.timeout = timeout_in_secs;
    }

    pub fn set_time_fn<T>(&mut self, time_fn: T)
    where
        T: Fn() -> Result<SystemTime, ManyError> + Send + Sync + 'static,
    {
        self.time_fn = Some(Arc::new(time_fn));
    }

    pub fn set_fallback_module<M>(&mut self, module: M) -> &mut Self
    where
        M: LowLevelManyRequestHandler + base::BaseModuleBackend + 'static,
    {
        self.fallback = Some(Arc::new(module));
        self
    }

    pub fn add_module<M>(&mut self, module: M) -> &mut Self
    where
        M: ManyModule + 'static,
    {
        let info = module.info();
        let ManyModuleInfo {
            attribute,
            endpoints,
            ..
        } = info;

        if let Some(Attribute { id, .. }) = attribute {
            if let Some(m) = self
                .modules
                .iter()
                .find(|m| m.info().attribute.as_ref().map(|x| x.id) == Some(*id))
            {
                panic!(
                    "Module {} already implements attribute {}.",
                    m.info().name,
                    id
                );
            }
        }

        for e in endpoints {
            if self.method_cache.contains(e.as_str()) {
                unreachable!(
                    "Method '{}' already implemented, but there was no attribute conflict.",
                    e
                );
            }
        }

        // Update the cache.
        for e in endpoints {
            self.method_cache.insert(e.clone());
        }
        self.modules.push(Arc::new(module));
        self
    }

    pub fn validate_id(&self, message: &RequestMessage) -> Result<(), ManyError> {
        let to = &message.to;

        // Verify that the message is for this server, if it's not anonymous.
        if to.is_anonymous() || &self.identity.identity == to {
            Ok(())
        } else {
            Err(ManyError::unknown_destination(
                to.to_string(),
                self.identity.identity.to_string(),
            ))
        }
    }

    pub fn find_module(&self, message: &RequestMessage) -> Option<Arc<dyn ManyModule + Send>> {
        self.modules
            .iter()
            .find(|x| x.info().endpoints.contains(&message.method))
            .cloned()
    }
}

impl Debug for ManyServer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManyServer").finish()
    }
}

impl base::BaseModuleBackend for ManyServer {
    fn endpoints(&self) -> Result<base::Endpoints, ManyError> {
        let mut endpoints: BTreeSet<String> = self.method_cache.iter().cloned().collect();

        if let Some(fb) = &self.fallback {
            endpoints = endpoints
                .union(&fb.endpoints()?.0.iter().cloned().collect::<BTreeSet<_>>())
                .cloned()
                .collect();
        }

        Ok(base::Endpoints(endpoints))
    }

    fn status(&self) -> Result<base::Status, ManyError> {
        let mut attributes: BTreeSet<Attribute> = self
            .modules
            .iter()
            .filter_map(|m| m.info().attribute.clone())
            .collect();

        let mut builder = base::StatusBuilder::default();

        builder
            .name(self.name.clone())
            .version(1)
            .identity(self.identity.identity)
            .timeout(self.timeout)
            .extras(BTreeMap::new());

        if let Some(pk) = self.identity.public_key() {
            builder.public_key(pk);
        }
        if let Some(sv) = self.version.clone() {
            builder.server_version(sv);
        }

        if let Some(fb) = &self.fallback {
            let fb_status = fb.status()?;
            if fb_status.identity != self.identity.identity
                || fb_status.version != 1
                || (fb_status.server_version != self.version && self.version.is_some())
            {
                tracing::error!(
                    "fallback status differs from internal status: {} != {} || {:?} != {:?}",
                    fb_status.identity,
                    self.identity.identity,
                    fb_status.server_version,
                    self.version
                );
                return Err(ManyError::internal_server_error());
            }

            if let Some(sv) = fb_status.server_version {
                builder.server_version(sv);
            }

            builder.name(fb_status.name).extras(fb_status.extras);

            attributes = attributes
                .into_iter()
                .chain(fb_status.attributes.into_iter())
                .collect();
        }

        builder.attributes(attributes.into_iter().collect());

        builder
            .build()
            .map_err(|x| ManyError::unknown(x.to_string()))
    }
}

#[async_trait]
impl LowLevelManyRequestHandler for Arc<Mutex<ManyServer>> {
    async fn execute(&self, envelope: CoseSign1) -> Result<CoseSign1, String> {
        let request = {
            let this = self.lock().unwrap();
            many_protocol::decode_request_from_cose_sign1(
                envelope.clone(),
                this.allowed_origins.clone(),
            )
        };
        let mut id = None;

        let response = {
            let this = self.lock().unwrap();
            let cose_id = this.identity.clone();

            (|| {
                let message = request?;

                let now = this
                    .time_fn
                    .as_ref()
                    .map_or_else(|| Ok(SystemTime::now()), |f| f())?;

                id = message.id;

                _validate_time(&message, now, this.timeout)?;

                this.validate_id(&message)?;

                let maybe_module = this.find_module(&message);
                if let Some(ref m) = maybe_module {
                    m.validate(&message, &envelope)?;
                };

                Ok((
                    cose_id.clone(),
                    message,
                    maybe_module,
                    this.fallback.clone(),
                ))
            })()
            .map_err(|many_err: ManyError| ResponseMessage::error(&cose_id.identity, id, many_err))
        };

        match response {
            Ok((cose_id, message, maybe_module, fallback)) => match (maybe_module, fallback) {
                (Some(m), _) => {
                    let mut response = match m.execute(message).await {
                        Ok(response) => response,
                        Err(many_err) => ResponseMessage::error(&cose_id.identity, id, many_err),
                    };
                    response.from = cose_id.identity;
                    many_protocol::encode_cose_sign1_from_response(response, &cose_id)
                }
                (None, Some(fb)) => {
                    LowLevelManyRequestHandler::execute(fb.as_ref(), envelope).await
                }
                (None, None) => {
                    let response = ResponseMessage::error(
                        &cose_id.identity,
                        id,
                        ManyError::could_not_route_message(),
                    );
                    many_protocol::encode_cose_sign1_from_response(response, &cose_id)
                }
            },
            Err(response) => {
                let this = self.lock().unwrap();
                many_protocol::encode_cose_sign1_from_response(response, &this.identity)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use semver::{BuildMetadata, Prerelease, Version};
    use std::ops::Sub;
    use std::sync::RwLock;
    use std::time::Duration;

    use super::*;
    use many_identity::cose_helpers::public_key;
    use many_identity::testsutils::generate_random_eddsa_identity;
    use many_identity::Address;
    use many_modules::base::Status;
    use many_protocol::{
        decode_response_from_cose_sign1, encode_cose_sign1_from_request, RequestMessageBuilder,
    };
    use many_types::Timestamp;
    use proptest::prelude::*;

    const ALPHA_NUM_DASH_REGEX: &str = "[a-zA-Z0-9-]";

    prop_compose! {
        fn arb_semver()((major, minor, patch) in (any::<u64>(), any::<u64>(), any::<u64>()), pre in ALPHA_NUM_DASH_REGEX, build in ALPHA_NUM_DASH_REGEX) -> Version {
            Version {
                major,
                minor,
                patch,
                pre: Prerelease::new(&pre).unwrap(),
                build: BuildMetadata::new(&build).unwrap(),
            }
        }
    }

    proptest! {
        #[test]
        fn simple_status(name in "\\PC*", version in arb_semver()) {
            let id = generate_random_eddsa_identity();
            let server = ManyServer::simple(name.clone(), id.clone(), Some(version.to_string()), None);

            // Test status() using a message instead of a direct call
            //
            // This will test other ManyServer methods as well
            let request: RequestMessage = RequestMessageBuilder::default()
                .version(1)
                .from(id.identity)
                .to(id.identity)
                .method("status".to_string())
                .data("null".as_bytes().to_vec())
                .build()
                .unwrap();

            let envelope = encode_cose_sign1_from_request(request, &id).unwrap();
            let response = smol::block_on(async { server.execute(envelope).await }).unwrap();
            let response_message = decode_response_from_cose_sign1(response, None).unwrap();

            let status: Status = minicbor::decode(&response_message.data.unwrap()).unwrap();

            assert_eq!(status.version, 1);
            assert_eq!(status.name, name);
            assert_eq!(status.public_key, Some(public_key(&id.key.unwrap()).unwrap()));
            assert_eq!(status.identity, id.identity);
            assert!(status.attributes.has_id(0));
            assert_eq!(status.server_version, Some(version.to_string()));
            assert_eq!(status.timeout, Some(MANYSERVER_DEFAULT_TIMEOUT));
            assert_eq!(status.extras, BTreeMap::new());
        }
    }

    #[test]
    fn validate_time() {
        let timestamp = SystemTime::now();
        let request: RequestMessage = RequestMessageBuilder::default()
            .version(1)
            .from(Address::anonymous())
            .to(Address::anonymous())
            .method("status".to_string())
            .data("null".as_bytes().to_vec())
            .timestamp(Timestamp::from_system_time(timestamp).unwrap())
            .build()
            .unwrap();

        // Okay with the same
        assert!(_validate_time(&request, timestamp, 100).is_ok());
        // Okay with the past
        assert!(_validate_time(&request, timestamp - Duration::from_secs(10), 100).is_ok());
        // Okay with the future
        assert!(_validate_time(&request, timestamp + Duration::from_secs(10), 100).is_ok());

        // NOT okay with the past too much
        assert!(_validate_time(&request, timestamp - Duration::from_secs(101), 100).is_err());
        // NOT okay with the future too much
        assert!(_validate_time(&request, timestamp + Duration::from_secs(101), 100).is_err());
    }

    #[test]
    fn server_manages_time() {
        fn create_request(timestamp: SystemTime, nonce: u8) -> CoseSign1 {
            let request: RequestMessage = RequestMessageBuilder::default()
                .method("status".to_string())
                .timestamp(Timestamp::from_system_time(timestamp).unwrap())
                .nonce(nonce.to_le_bytes().to_vec())
                .build()
                .unwrap();
            encode_cose_sign1_from_request(request, &CoseKeyIdentity::anonymous()).unwrap()
        }

        let server = ManyServer::simple("test-server", CoseKeyIdentity::anonymous(), None, None);
        let timestamp = SystemTime::now();
        let now = Arc::new(RwLock::new(timestamp));
        let get_now = {
            let n = now.clone();
            move || Ok(*n.read().unwrap())
        };

        // timestamp is now, so this should be fairly close to it and should pass.
        let response_e = smol::block_on(server.execute(create_request(timestamp, 0))).unwrap();
        let response = decode_response_from_cose_sign1(response_e, None).unwrap();
        assert!(response.data.is_ok());

        // Set time to present.
        {
            server.lock().unwrap().set_time_fn(get_now);
        }
        let response_e = smol::block_on(server.execute(create_request(timestamp, 1))).unwrap();
        let response = decode_response_from_cose_sign1(response_e, None).unwrap();
        assert!(response.data.is_ok());

        // Set time to 10 minutes past.
        {
            *now.write().unwrap() = timestamp.sub(Duration::from_secs(60 * 60 * 10));
        }
        let response_e = smol::block_on(server.execute(create_request(timestamp, 2))).unwrap();
        let response = decode_response_from_cose_sign1(response_e, None).unwrap();
        assert!(response.data.is_err());
        assert_eq!(
            response.data.unwrap_err().code(),
            ManyError::timestamp_out_of_range().code()
        );

        // Set request timestamp 10 minutes in the past.
        let response_e = smol::block_on(server.execute(create_request(
            timestamp.sub(Duration::from_secs(60 * 60 * 10)),
            3,
        )))
        .unwrap();
        let response = decode_response_from_cose_sign1(response_e, None).unwrap();
        assert!(response.data.is_ok());
    }
}

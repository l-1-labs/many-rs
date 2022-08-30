use anyhow::anyhow;
use async_recursion::async_recursion;
use clap::{ArgGroup, Parser};
use coset::{CborSerializable, CoseSign1};
use many_client::ManyClient;
use many_error::ManyError;
use many_identity::verifiers::AnonymousVerifier;
use many_identity::{Address, AnonymousIdentity, Identity};
use many_identity_dsa::{CoseKeyIdentity, CoseKeyVerifier};
use many_identity_hsm::{Hsm, HsmIdentity, HsmMechanismType, HsmSessionType, HsmUserType};
use many_mock::{parse_mockfile, server::ManyMockServer, MockEntries};
use many_modules::ledger;
use many_modules::r#async::attributes::AsyncAttribute;
use many_modules::r#async::{StatusArgs, StatusReturn};
use many_protocol::{
    encode_cose_sign1_from_request, RequestMessage, RequestMessageBuilder, ResponseMessage,
};
use many_server::transport::http::HttpServer;
use many_server::ManyServer;
use many_types::Timestamp;
use std::convert::TryFrom;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{error, info, level_filters::LevelFilter, trace};
use url::Url;

#[derive(Parser)]
struct Opts {
    /// Increase output logging verbosity to DEBUG level.
    #[clap(short, long, parse(from_occurrences))]
    verbose: i8,

    /// Suppress all output logging. Can be used multiple times to suppress more.
    #[clap(short, long, parse(from_occurrences))]
    quiet: i8,

    #[clap(subcommand)]
    subcommand: SubCommand,
}

#[derive(Parser)]
enum SubCommand {
    /// Transform a textual ID into its hexadecimal value, or the other way around.
    /// If the argument is neither hexadecimal value or identity, try to see if it's
    /// a file, and will parse it as a PEM file.
    Id(IdOpt),

    /// Display the textual ID of a public key located on an HSM.
    HsmId(HsmIdOpt),

    /// Creates a message and output it.
    Message(MessageOpt),

    /// Starts a base server that can also be used for reverse proxying
    /// to another MANY server.
    Server(ServerOpt),

    /// Get the token ID per string of a ledger's token.
    GetTokenId(GetTokenIdOpt),
}

#[derive(Parser)]
struct IdOpt {
    /// An hexadecimal value to encode, an identity textual format to decode or
    /// a PEM file to read
    arg: String,

    /// Allow to generate the identity with a specific subresource ID.
    subid: Option<u32>,
}

#[derive(Parser)]
struct HsmIdOpt {
    /// HSM PKCS#11 module path
    module: PathBuf,

    /// HSM PKCS#11 slot ID
    slot: u64,

    /// HSM PKCS#11 key ID
    keyid: String,

    /// Allow to generate the identity with a specific subresource ID.
    subid: Option<u32>,
}

#[derive(Parser)]
#[clap(
    group(
        ArgGroup::new("hsm")
            .multiple(true)
            .args(&["module", "slot", "keyid"])
            .requires_all(&["module", "slot", "keyid"])
    ),
    group(
        ArgGroup::new("action")
            .args(&["server", "hex", "base64"])
            .required(true)
    )
)]
struct MessageOpt {
    /// A pem file to sign the message. If this is omitted, the message will be anonymous.
    #[clap(long)]
    pem: Option<PathBuf>,

    /// Timestamp (in seconds since epoch).
    #[clap(long)]
    timestamp: Option<u64>,

    /// The server to connect to.
    #[clap(long)]
    server: Option<Url>,

    /// If true, prints out the hex value of the message bytes.
    #[clap(long)]
    hex: bool,

    /// If true, prints out the base64 value of the message bytes.
    #[clap(long)]
    base64: bool,

    /// If used, send the message from hexadecimal to the server and wait for
    /// the response.
    #[clap(long, requires("server"))]
    from_hex: Option<String>,

    /// Show the async token and exit right away. By default, will poll for the
    /// result of the async operation.
    #[clap(long)]
    r#async: bool,

    /// The identity to send it to.
    #[clap(long)]
    to: Option<Address>,

    /// HSM PKCS#11 module path
    #[clap(long, conflicts_with("pem"))]
    module: Option<PathBuf>,

    /// HSM PKCS#11 slot ID
    #[clap(long, conflicts_with("pem"))]
    slot: Option<u64>,

    /// HSM PKCS#11 key ID
    #[clap(long, conflicts_with("pem"))]
    keyid: Option<String>,

    /// The method to call.
    method: Option<String>,

    /// The content of the message itself (its payload).
    data: Option<String>,
}

#[derive(Parser)]
struct ServerOpt {
    /// The location of a PEM file for the identity of this server.
    #[clap(long)]
    pem: PathBuf,

    /// The address and port to bind to for the MANY Http server.
    #[clap(long, short, default_value = "127.0.0.1:8000")]
    addr: SocketAddr,

    /// The name to give the server.
    #[clap(long, short, default_value = "many-server")]
    name: String,

    /// The path to a mockfile containing mock responses.
    /// Default is mockfile.toml, gives an error if the file does not exist
    #[clap(long, short, value_parser = parse_mockfile)]
    mockfile: Option<MockEntries>,
}

#[derive(Parser)]
struct GetTokenIdOpt {
    /// The server to call. It MUST implement the ledger attribute (2).
    server: url::Url,

    /// The token to get. If not listed in the list of tokens, this will
    /// error.
    symbol: String,
}

#[async_recursion(?Send)]
async fn show_response<'a>(
    response: &'a ResponseMessage,
    client: ManyClient<impl Identity + 'a>,
    r#async: bool,
) -> Result<(), anyhow::Error> {
    let ResponseMessage {
        data, attributes, ..
    } = response;

    let payload = data.clone()?;
    if payload.is_empty() {
        let attr = attributes.get::<AsyncAttribute>().unwrap();
        info!("Async token: {}", hex::encode(&attr.token));

        // Allow eprint/ln for showing the progress bar, when we're interactive.
        #[allow(clippy::print_stderr)]
        fn progress(str: &str, done: bool) {
            if atty::is(atty::Stream::Stderr) {
                if done {
                    eprintln!("{}", str);
                } else {
                    eprint!("{}", str);
                }
            }
        }

        if !r#async {
            progress("Waiting.", false);

            // TODO: improve on this by using duration and thread and watchdog.
            // Wait for the server for ~60 seconds by pinging it every second.
            for _ in 0..60 {
                let response = client
                    .call(
                        "async.status",
                        StatusArgs {
                            token: attr.token.clone(),
                        },
                    )
                    .await?;
                let status: StatusReturn = minicbor::decode(&response.data?)?;
                match status {
                    StatusReturn::Done { response } => {
                        progress(".", true);
                        return show_response(&*response, client, r#async).await;
                    }
                    StatusReturn::Expired => {
                        progress(".", true);
                        info!("Async token expired before we could check it.");
                        return Ok(());
                    }
                    _ => {
                        progress(".", false);
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        }
    } else {
        println!(
            "{}",
            cbor_diag::parse_bytes(&payload).unwrap().to_diag_pretty()
        );
    }

    Ok(())
}

async fn message(
    s: Url,
    to: Address,
    key: impl Identity,
    method: String,
    data: Vec<u8>,
    timestamp: Option<SystemTime>,
    r#async: bool,
) -> Result<(), anyhow::Error> {
    let address = key.address();
    let client = ManyClient::new(s, to, key).unwrap();

    let mut nonce = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce);

    let mut builder = many_protocol::RequestMessageBuilder::default();
    builder
        .version(1)
        .from(address)
        .to(to)
        .method(method)
        .data(data)
        .nonce(nonce.to_vec());

    if let Some(ts) = timestamp {
        builder.timestamp(Timestamp::from_system_time(ts)?);
    }

    let message: RequestMessage = builder
        .build()
        .map_err(|_| ManyError::internal_server_error())?;

    let response = client.send_message(message).await?;

    show_response(&response, client, r#async).await
}

async fn message_from_hex(
    s: Url,
    to: Address,
    key: impl Identity,
    hex: String,
    r#async: bool,
) -> Result<(), anyhow::Error> {
    let client = ManyClient::new(s.clone(), to, key).unwrap();

    let data = hex::decode(hex)?;
    let envelope = CoseSign1::from_slice(&data).map_err(|e| anyhow!(e))?;

    let cose_sign1 = many_client::client::send_envelope(s, envelope).await?;
    let response =
        ResponseMessage::decode_and_verify(&cose_sign1, &(AnonymousVerifier, CoseKeyVerifier))
            .map_err(|e| anyhow!(e))?;

    show_response(&response, client, r#async).await
}

#[tokio::main]
async fn main() {
    let Opts {
        verbose,
        quiet,
        subcommand,
    } = Opts::parse();
    let verbose_level = 2 + verbose - quiet;
    let log_level = match verbose_level {
        x if x > 3 => LevelFilter::TRACE,
        3 => LevelFilter::DEBUG,
        2 => LevelFilter::INFO,
        1 => LevelFilter::WARN,
        0 => LevelFilter::ERROR,
        x if x < 0 => LevelFilter::OFF,
        _ => unreachable!(),
    };
    tracing_subscriber::fmt().with_max_level(log_level).init();

    match subcommand {
        SubCommand::Id(o) => {
            if let Ok(data) = hex::decode(&o.arg) {
                match Address::try_from(data.as_slice()) {
                    Ok(mut i) => {
                        if let Some(subid) = o.subid {
                            i = i
                                .with_subresource_id(subid)
                                .expect("Invalid subresource id");
                        }
                        println!("{}", i)
                    }
                    Err(e) => {
                        error!("Identity did not parse: {:?}", e.to_string());
                        std::process::exit(1);
                    }
                }
            } else if let Ok(mut i) = Address::try_from(o.arg.clone()) {
                if let Some(subid) = o.subid {
                    i = i
                        .with_subresource_id(subid)
                        .expect("Invalid subresource id");
                }
                println!("{}", hex::encode(&i.to_vec()));
            } else if let Ok(pem_content) = std::fs::read_to_string(&o.arg) {
                // Create the identity from the public key hash.
                let mut i = CoseKeyIdentity::from_pem(&pem_content).unwrap().address();
                if let Some(subid) = o.subid {
                    i = i
                        .with_subresource_id(subid)
                        .expect("Invalid subresource id");
                }

                println!("{}", i);
            } else {
                error!("Could not understand the argument.");
                std::process::exit(2);
            }
        }
        SubCommand::HsmId(o) => {
            let keyid = hex::decode(o.keyid).expect("Failed to decode keyid to hex");

            {
                let mut hsm = Hsm::get_instance().expect("HSM mutex poisoned");
                hsm.init(o.module, keyid)
                    .expect("Failed to initialize HSM module");

                // The session will stay open until the application terminates
                hsm.open_session(o.slot, HsmSessionType::RO, None, None)
                    .expect("Failed to open HSM session");
            }

            let mut id = HsmIdentity::new(HsmMechanismType::ECDSA)
                .expect("Unable to create CoseKeyIdentity from HSM")
                .address();

            if let Some(subid) = o.subid {
                id = id
                    .with_subresource_id(subid)
                    .expect("Invalid subresource id");
            }

            println!("{}", id);
        }
        SubCommand::Message(o) => {
            let to_identity = o.to.unwrap_or_default();
            let timestamp = o.timestamp.map(|secs| {
                SystemTime::UNIX_EPOCH
                    .checked_add(Duration::new(secs, 0))
                    .expect("Invalid timestamp")
            });
            let data = o
                .data
                .map_or(vec![], |d| cbor_diag::parse_diag(&d).unwrap().to_bytes());

            let from_identity: Box<dyn Identity> = if let (Some(module), Some(slot), Some(keyid)) =
                (o.module, o.slot, o.keyid)
            {
                trace!("Getting user PIN");
                let pin = rpassword::prompt_password("Please enter the HSM user PIN: ")
                    .expect("I/O error when reading HSM PIN");
                let keyid = hex::decode(keyid).expect("Failed to decode keyid to hex");

                {
                    let mut hsm = Hsm::get_instance().expect("HSM mutex poisoned");
                    hsm.init(module, keyid)
                        .expect("Failed to initialize HSM module");

                    // The session will stay open until the application terminates
                    hsm.open_session(slot, HsmSessionType::RO, Some(HsmUserType::User), Some(pin))
                        .expect("Failed to open HSM session");
                }

                // Only ECDSA is supported at the moment. It should be easy to add support for
                // new EC mechanisms.
                Box::new(
                    HsmIdentity::new(HsmMechanismType::ECDSA)
                        .expect("Unable to create CoseKeyIdentity from HSM"),
                )
            } else if let Some(p) = o.pem {
                // If `pem` is not provided, use anonymous and don't sign.
                Box::new(CoseKeyIdentity::from_pem(&std::fs::read_to_string(&p).unwrap()).unwrap())
            } else {
                Box::new(AnonymousIdentity)
            };

            if let Some(s) = o.server {
                let result = if let Some(hex) = o.from_hex {
                    message_from_hex(s, to_identity, from_identity, hex, o.r#async).await
                } else {
                    message(
                        s,
                        to_identity,
                        from_identity,
                        o.method.expect("--method is required"),
                        data,
                        timestamp,
                        o.r#async,
                    )
                    .await
                };

                match result {
                    Ok(()) => {}
                    Err(err) => {
                        error!(
                            "Error returned by server:\n|  {}\n",
                            err.to_string()
                                .split('\n')
                                .collect::<Vec<&str>>()
                                .join("\n|  ")
                        );
                        std::process::exit(1);
                    }
                }
            } else {
                let message: RequestMessage = RequestMessageBuilder::default()
                    .version(1)
                    .from(from_identity.address())
                    .to(to_identity)
                    .method(o.method.expect("--method is required"))
                    .data(data)
                    .build()
                    .unwrap();

                let cose = encode_cose_sign1_from_request(message, &from_identity).unwrap();
                let bytes = cose.to_vec().unwrap();
                if o.hex {
                    println!("{}", hex::encode(&bytes));
                } else if o.base64 {
                    println!("{}", base64::encode(&bytes));
                } else {
                    panic!("Must specify one of hex, base64 or server...");
                }
            }
        }
        SubCommand::Server(o) => {
            let pem = std::fs::read_to_string(&o.pem).expect("Could not read PEM file.");
            let key = Arc::new(
                CoseKeyIdentity::from_pem(&pem)
                    .expect("Could not generate identity from PEM file."),
            );

            let many = ManyServer::simple(
                o.name,
                Arc::clone(&key),
                (AnonymousVerifier, CoseKeyVerifier),
                Some(std::env!("CARGO_PKG_VERSION").to_string()),
            );
            let mockfile = o.mockfile.unwrap_or_default();
            if !mockfile.is_empty() {
                let mut many_locked = many.lock().unwrap();
                let mock_server = ManyMockServer::new(mockfile, None, key);
                many_locked.set_fallback_module(mock_server);
            }
            HttpServer::new(many).bind(o.addr).unwrap();
        }
        SubCommand::GetTokenId(o) => {
            let client = ManyClient::new(o.server, Address::anonymous(), AnonymousIdentity)
                .expect("Could not create a client");
            let status = client.status().await.expect("Cannot get status of server");

            if !status.attributes.contains(&ledger::LEDGER_MODULE_ATTRIBUTE) {
                error!("Server does not implement Ledger Attribute.");
                process::exit(1);
            }

            let info: ledger::InfoReturns = minicbor::decode(
                &client
                    .call("ledger.info", ledger::InfoArgs {})
                    .await
                    .unwrap()
                    .data
                    .expect("An error happened during the call to ledger.info"),
            )
            .expect("Invalid data returned by server; not CBOR");

            let symbol = o.symbol;
            let id = info
                .local_names
                .into_iter()
                .find(|(_, y)| y == &symbol)
                .map(|(x, _)| x)
                .ok_or_else(|| format!("Could not resolve symbol '{}'", &symbol))
                .unwrap();

            println!("{}", id);
        }
    }
}

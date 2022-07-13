use crate::server::module::EmptyArg;
use crate::types::events::EventKind;
use minicbor::{Decode, Encode};

pub type InfoArgs = EmptyArg;

#[derive(Decode, Encode)]
#[cbor(map)]
pub struct InfoReturn {
    #[n(0)]
    pub total: u64,

    #[n(1)]
    pub event_types: Vec<EventKind>,
}

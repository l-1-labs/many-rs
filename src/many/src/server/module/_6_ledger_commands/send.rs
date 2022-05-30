use crate::{types::ledger, server::module::EmptyReturn};
use crate::Identity;
use minicbor::{Decode, Encode};

#[derive(Debug, Clone, Encode, Decode)]
#[cfg_attr(test, derive(PartialEq))]
#[cbor(map)]
pub struct SendArgs {
    #[n(0)]
    pub from: Option<Identity>,

    #[n(1)]
    pub to: Identity,

    #[n(2)]
    pub amount: ledger::TokenAmount,

    #[n(3)]
    pub symbol: ledger::Symbol,
}

pub type SendReturns = EmptyReturn;

use many_types::web::{WebDeploymentFilter, WebDeploymentInfo};
use many_types::SortOrder;
use minicbor::{Decode, Encode};

#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq)]
#[cbor(map)]
pub struct ListArgs {
    #[n(0)]
    pub count: Option<usize>,

    #[n(1)]
    pub order: Option<SortOrder>,

    #[n(2)]
    pub filter: Option<Vec<WebDeploymentFilter>>,
}

#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq)]
#[cbor(map)]
pub struct ListReturns {
    #[n(0)]
    pub deployments: Vec<WebDeploymentInfo>,
}

use std::time::Duration;

use yi_oauth::store::{self, Credential, Kind};

pub const ID_TOKEN: &str =
    "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC03In19.sig";

pub fn expired(org: Option<&str>) -> Credential {
    Credential {
        kind: Kind::Oauth,
        access: "old-access".to_owned(),
        refresh: Some("old-refresh".to_owned()),
        expires: Some(store::now() - Duration::from_secs(60)),
        account: None,
        org: org.map(str::to_owned),
        extra: Default::default(),
    }
}

use psh_engine::host::HostInfo;
use std::{future::Future, pin::Pin, time::Duration};

pub trait Machine {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()>>>;
    fn random(&self, bytes: &mut [u8]);

    fn utc_offset(&self, epoch_secs: i64) -> i32 {
        let _ = epoch_secs;
        0
    }
}

#[derive(Clone, Debug)]
pub struct Identity {
    pub info: HostInfo,
    pub pid: u32,
    pub user: Vec<u8>,
    pub group: Vec<u8>,
    pub home: Vec<u8>,
}

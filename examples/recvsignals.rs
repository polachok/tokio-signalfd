use tokio::prelude::*;
use tokio_signalfd::{SignalFd, SIGINT, SIGTERM};

fn main() {
    let signals = SignalFd::new(&[SIGINT, SIGTERM]).unwrap();
    tokio::run(future::lazy(move || {
        signals
            .for_each(|signal| {
                println!("received signal#{}", signal);
                Ok(())
            })
            .map_err(|err| panic!("{:?}", err))
    }))
}

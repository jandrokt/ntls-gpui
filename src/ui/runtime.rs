//! The bridge between GPUI's foreground executor and the tokio runtime the
//! tools run on.
//!
//! Nothing in a tool touches the interface: it emits events into a channel and
//! the window drains that channel in batches. That is what lets a scan run at
//! full speed without the repaints becoming the bottleneck.

use std::sync::OnceLock;

use tokio::runtime::Runtime;

/// The runtime every tool runs on. One multi-threaded pool serves every open
/// job, so a subnet sweep and a port scan share the same threads rather than
/// each demanding their own.
pub fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("ntls")
            .build()
            .expect("cannot start the tokio runtime")
    })
}

// Include generated code from OUT_DIR (set by cargo during build)
// prost generates filename based on proto package name
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/zellij.remote.v1.rs"));
}

pub use proto::*;

#[cfg(test)]
mod tests;

pub const ZRP_VERSION_MAJOR: u32 = 1;
pub const ZRP_VERSION_MINOR: u32 = 0;
pub const DEFAULT_MAX_DATAGRAM_BYTES: u32 = 1200;
pub const DEFAULT_RENDER_WINDOW: u32 = 4;

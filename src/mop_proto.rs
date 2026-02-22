#[cfg(feature = "mop-proto")]
pub mod mop {
    pub const DESCRIPTOR_SET_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mop_descriptor.bin"));
    include!(concat!(env!("OUT_DIR"), "/proto.rs"));
}

#[cfg(not(feature = "mop-proto"))]
pub mod mop {}
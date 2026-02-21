#[cfg(feature = "mop-proto")]
pub mod mop {
    include!(concat!(env!("OUT_DIR"), "/proto.rs"));
}

#[cfg(not(feature = "mop-proto"))]
pub mod mop {}
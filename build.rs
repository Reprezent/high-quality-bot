use std::env;
use std::path::PathBuf;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn main() {
    let build_timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("failed to format build timestamp");
    println!("cargo:rustc-env=BUILD_TIMESTAMP_UTC={build_timestamp}");

    println!("cargo:rerun-if-env-changed=MOP_PROTO_DIR");

    if env::var_os("CARGO_FEATURE_MOP_PROTO").is_none() {
        return;
    }

    let proto_dir = env::var("MOP_PROTO_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("vendor/wowsims-mop/proto"));

    println!("cargo:rerun-if-changed={}", proto_dir.display());

    if !proto_dir.exists() {
        panic!(
            "Proto directory not found at {}. Add the submodule (vendor/wowsims-mop) or set MOP_PROTO_DIR.",
            proto_dir.display()
        );
    }

    let proto_dir = std::fs::canonicalize(&proto_dir)
        .unwrap_or_else(|err| panic!("Unable to canonicalize proto dir {}: {err}", proto_dir.display()));

    let api_proto = proto_dir.join("api.proto");
    if !api_proto.exists() {
        panic!("api.proto not found in {}", proto_dir.display());
    }

    let mut includes = vec![proto_dir.clone()];
    let system_include = PathBuf::from("/usr/include");
    if system_include.exists() {
        includes.push(system_include);
    }

    let out_dir = PathBuf::from(
        env::var("OUT_DIR").expect("OUT_DIR environment variable missing during build"),
    );

    let mut config = prost_build::Config::new();
    config.file_descriptor_set_path(out_dir.join("mop_descriptor.bin"));
    config
        .compile_protos(&[api_proto], &includes)
        .expect("failed to compile wowsims/mop protobuf files");
}
use std::env;
use std::path::{Path, PathBuf};

fn main() {
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

    let proto_files = collect_proto_files(&proto_dir);
    if proto_files.is_empty() {
        panic!("No .proto files found in {}", proto_dir.display());
    }

    prost_build::Config::new()
        .compile_protos(&proto_files, &[proto_dir])
        .expect("failed to compile wowsims/mop protobuf files");
}

fn collect_proto_files(proto_dir: &Path) -> Vec<PathBuf> {
    let mut files = std::fs::read_dir(proto_dir)
        .unwrap_or_else(|err| panic!("Unable to read {}: {err}", proto_dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("proto"))
        .collect::<Vec<_>>();

    files.sort();
    files
}
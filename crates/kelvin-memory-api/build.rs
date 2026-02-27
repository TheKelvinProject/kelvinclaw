fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc_path);

    let proto_file = "proto/kelvin/memory/v1alpha1/memory.proto";
    let descriptor_path =
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("kelvin_memory_descriptor.bin");
    println!("cargo:rerun-if-changed={proto_file}");

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .file_descriptor_set_path(descriptor_path)
        .compile_protos(&[proto_file], &["proto"])?;

    Ok(())
}

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (proto_root, proto_prefix, libs_prefix) =
        if Path::new("../protobuf/identity/identity.proto").exists() {
            ("..", "../protobuf", "../libs")
        } else {
            (".", "protobuf", "libs")
        };

    let files = vec![
        format!("{proto_prefix}/identity/identity.proto"),
        format!("{proto_prefix}/shared/user/user.proto"),
        format!("{proto_prefix}/shared/organization/organization.proto"),
        format!("{libs_prefix}/protobuf/common/base.proto"),
    ];
    let file_refs: Vec<&str> = files.iter().map(String::as_str).collect();

    tonic_build::configure()
        .build_server(false)
        .compile_protos(&file_refs, &[proto_root])?;
    Ok(())
}

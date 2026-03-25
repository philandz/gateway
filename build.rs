fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &[
                "../protobuf/identity/identity.proto",
                "../protobuf/shared/user/user.proto",
                "../protobuf/shared/organization/organization.proto",
                "../libs/protobuf/common/base.proto",
            ],
            &[".."],
        )?;
    Ok(())
}

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Determine where we are being built from.
    //
    // The gateway ships its own copy of portfolio.proto locally under
    // `gateway/protobuf/portfolio/`.  All other protos live at the repo-root
    // `protobuf/` level.  We detect the build context by testing for the
    // local portfolio proto.
    let at_gateway_dir = Path::new("protobuf/portfolio/portfolio.proto").exists();

    // proto_root is the include path that covers the majority of protos.
    // libs_prefix covers protos under libs/protobuf/.
    let (proto_root, libs_prefix) = if at_gateway_dir {
        ("..", "../libs")
    } else {
        (".", "libs")
    };

    let mut includes = vec![proto_root.to_string()];
    // When building from the gateway directory we also add "." as an include so
    // that the local `gateway/protobuf/portfolio/portfolio.proto` can be found.
    if at_gateway_dir {
        includes.push(".".to_string());
    }
    for candidate in [
        "/usr/include",
        "/usr/local/include",
        "/opt/homebrew/include",
    ] {
        if Path::new(candidate).exists() {
            includes.push(candidate.to_string());
        }
    }

    if let Ok(extra_include) = std::env::var("PROTOC_INCLUDE") {
        if !extra_include.is_empty() && Path::new(&extra_include).exists() {
            includes.push(extra_include);
        }
    }

    let include_refs: Vec<&str> = includes.iter().map(String::as_str).collect();

    // All protos except portfolio live at proto_root; portfolio lives in the
    // gateway's local protobuf/ directory (resolved via the "." include above).
    let files = if at_gateway_dir {
        vec![
            format!("{}/protobuf/identity/identity.proto", proto_root),
            format!("{}/protobuf/media/media.proto", proto_root),
            format!("{}/protobuf/budget/budget.proto", proto_root),
            format!("{}/protobuf/category/category.proto", proto_root),
            format!("{}/protobuf/entry/entry.proto", proto_root),
            format!("{}/protobuf/sharing/sharing.proto", proto_root),
            // portfolio.proto is the only proto that lives inside the gateway crate
            "protobuf/portfolio/portfolio.proto".to_string(),
            format!("{}/protobuf/shared/user/user.proto", proto_root),
            format!("{}/protobuf/shared/organization/organization.proto", proto_root),
            format!("{}/protobuf/shared/media/media.proto", proto_root),
            format!("{}/protobuf/common/base.proto", libs_prefix),
        ]
    } else {
        vec![
            format!("{}/protobuf/identity/identity.proto", proto_root),
            format!("{}/protobuf/media/media.proto", proto_root),
            format!("{}/protobuf/budget/budget.proto", proto_root),
            format!("{}/protobuf/category/category.proto", proto_root),
            format!("{}/protobuf/entry/entry.proto", proto_root),
            format!("{}/protobuf/sharing/sharing.proto", proto_root),
            format!("{}/protobuf/portfolio/portfolio.proto", proto_root),
            format!("{}/protobuf/shared/user/user.proto", proto_root),
            format!("{}/protobuf/shared/organization/organization.proto", proto_root),
            format!("{}/protobuf/shared/media/media.proto", proto_root),
            format!("{}/protobuf/common/base.proto", libs_prefix),
        ]
    };
    let file_refs: Vec<&str> = files.iter().map(String::as_str).collect();

    tonic_build::configure()
        .build_server(true)
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Make string fields optional in JSON deserialization so callers can
        // omit fields the gateway will fill from path params (e.g. budget_id).
        .type_attribute(
            ".service.portfolio.CreateSavingsAccountRequest",
            "#[serde(default)]",
        )
        .type_attribute(
            ".service.portfolio.CreateFixedDepositRequest",
            "#[serde(default)]",
        )
        .type_attribute(
            ".service.portfolio.CreateGoldLotRequest",
            "#[serde(default)]",
        )
        .type_attribute(
            ".service.portfolio.CreateStockLotRequest",
            "#[serde(default)]",
        )
        .type_attribute(
            ".service.portfolio.CreateEtfLotRequest",
            "#[serde(default)]",
        )
        .type_attribute(
            ".service.portfolio.CreateCryptoLotRequest",
            "#[serde(default)]",
        )
        .type_attribute(
            ".service.portfolio.RecordPriceObservationRequest",
            "#[serde(default)]",
        )
        .type_attribute(
            ".service.portfolio.RecordStockDisposalRequest",
            "#[serde(default)]",
        )
        .compile_protos(&file_refs, &include_refs)?;
    Ok(())
}

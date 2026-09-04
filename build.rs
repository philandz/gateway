use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Determine where we are being built from.
    //
    // All protos live at the repo-root `protobuf/` level.  When building
    // from the gateway directory (which has its own Cargo.toml), we need to
    // use ".." as proto_root to reach the repo-level protos.
    let proto_root = if Path::new("../protobuf/budget/budget.proto").exists() {
        ".."
    } else {
        "."
    };
    let libs_prefix = if proto_root == ".." {
        ".."
    } else {
        "libs"
    };

    let mut includes = vec![proto_root.to_string()];
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

    let files = vec![
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
        format!("{}/libs/protobuf/common/base.proto", libs_prefix),
    ];
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

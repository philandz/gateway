use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (proto_root, proto_prefix, libs_prefix) =
        if Path::new("../protobuf/identity/identity.proto").exists() {
            ("..", "../protobuf", "../libs")
        } else {
            (".", "protobuf", "libs")
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

    let files = [
        format!("{proto_prefix}/identity/identity.proto"),
        format!("{proto_prefix}/media/media.proto"),
        format!("{proto_prefix}/budget/budget.proto"),
        format!("{proto_prefix}/category/category.proto"),
        format!("{proto_prefix}/entry/entry.proto"),
        format!("{proto_prefix}/sharing/sharing.proto"),
        format!("{proto_prefix}/portfolio/portfolio.proto"),
        format!("{proto_prefix}/shared/user/user.proto"),
        format!("{proto_prefix}/shared/organization/organization.proto"),
        format!("{proto_prefix}/shared/media/media.proto"),
        format!("{libs_prefix}/protobuf/common/base.proto"),
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

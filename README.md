# gateway

API gateway for Philand services.

## Responsibilities

- Public HTTP entrypoint for clients
- Route forwarding to legacy upstream and extracted services
- Identity HTTP proxy or gRPC transcoding mode
- Unified Swagger/OpenAPI exposure

## Runtime Endpoints

- Gateway bind: `HOST:PORT` (default `0.0.0.0:3000`)
- Health: `GET /health`
- API base: `/api/...`

## Identity Routing Modes

- `IDENTITY_TRANSPORT=proxy_http`: forward identity requests to `IDENTITY_URL`
- `IDENTITY_TRANSPORT=grpc_transcode`: map identity HTTP routes to identity gRPC

## Local Run

```bash
cargo run
```

Required env:

- `UPSTREAM_URL`
- `IDENTITY_GRPC_URL`

Optional env:

- `IDENTITY_URL`
- `IDENTITY_TRANSPORT`
- `HOST`, `PORT`

See `../libs/configs/README.md` for full config contract.

## Testing

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## Notes

- Keep public ingress/API exposure in gateway; internal services stay behind it.

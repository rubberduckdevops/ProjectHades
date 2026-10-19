# Hades

Personal apps platform. Currently one application: **Journal**.

| Dir | What |
|---|---|
| `backend/` | Rust/Axum API (runs on Lambda via `lambda_http`, or locally with `cargo run`) backed by DynamoDB |
| `frontend/` | React + Cloudscape SPA with Cognito sign-in (Amplify) |
| `infra/` | AWS CDK stack: DynamoDB, Cognito, Lambda + HTTP API, S3 + CloudFront |

See [`API_CONTRACT.md`](API_CONTRACT.md) for the API, table layout and config, and [`infra/README.md`](infra/README.md) for deploy and local-dev instructions.

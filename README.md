# Hades

Personal apps platform: **Journal** (timestamped daily log), **Notes** (Markdown notes with semantic search) and **Todos** (a task list with optional projects and due dates).

| Dir | What |
|---|---|
| `backend/` | Rust/Axum API (runs on Lambda via `lambda_http`, or locally with `cargo run`) backed by DynamoDB |
| `frontend/` | React + Cloudscape SPA with Cognito sign-in (Amplify) |
| `infra/` | AWS CDK stack: DynamoDB (+ stream), Cognito, API and indexer Lambdas + HTTP API, notes S3 bucket, S3 Vectors, S3 + CloudFront |

See [`API_CONTRACT.md`](API_CONTRACT.md) for the API, table layout and config, and [`infra/README.md`](infra/README.md) for deploy and local-dev instructions.

How notes search works, and why it's built this way: [`docs/vector-search.md`](docs/vector-search.md).

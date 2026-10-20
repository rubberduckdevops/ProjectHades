//! The Hades API binary: Axum on Lambda (`lambda_http`) or as a local TCP server.

use std::sync::Arc;

use backend::{
    AppState, auth::JwksVerifier, build_app, config::Config, embed::BedrockEmbedder, init_tracing,
    vectors::S3VectorStore,
};

#[tokio::main]
async fn main() -> Result<(), lambda_http::Error> {
    let on_lambda = std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some();
    init_tracing(on_lambda);

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(1);
        }
    };

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(config.region.clone()))
        .load()
        .await;
    let state = Arc::new(AppState {
        table_name: config.table_name.clone(),
        ddb: aws_sdk_dynamodb::Client::new(&aws_config),
        s3: aws_sdk_s3::Client::new(&aws_config),
        notes_bucket: config.notes_bucket.clone(),
        auth: Arc::new(JwksVerifier::new(
            &config.region,
            &config.user_pool_id,
            &config.client_id,
        )),
        embedder: Arc::new(BedrockEmbedder::new(
            aws_sdk_bedrockruntime::Client::new(&aws_config),
            &config.embed_model_id,
        )),
        vectors: Arc::new(S3VectorStore::new(
            aws_sdk_s3vectors::Client::new(&aws_config),
            &config.vector_bucket,
            &config.vector_index,
        )),
    });
    let app = build_app(state, &config.cors_origins);

    if on_lambda {
        tracing::info!(table = %config.table_name, bucket = %config.notes_bucket, "starting Lambda runtime");
        lambda_http::run(app).await
    } else {
        let addr = format!("0.0.0.0:{}", config.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!(table = %config.table_name, bucket = %config.notes_bucket, "listening on http://localhost:{}/api", config.port);
        axum::serve(listener, app).await?;
        Ok(())
    }
}

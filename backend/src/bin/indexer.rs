//! The search indexer Lambda: consumes the table's DynamoDB stream (partial batch failures
//! via `batchItemFailures`), or reindexes everything stale when invoked with
//! `{"backfill": true}`. The logic lives in `backend::indexer`.

use std::sync::Arc;

use backend::{
    config::IndexerConfig, embed::BedrockEmbedder, indexer::Indexer, init_tracing,
    vectors::S3VectorStore,
};
use lambda_runtime::{Error, LambdaEvent, service_fn};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init_tracing(std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some());

    let config = match IndexerConfig::from_env() {
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
    let indexer = Arc::new(Indexer {
        ddb: aws_sdk_dynamodb::Client::new(&aws_config),
        s3: aws_sdk_s3::Client::new(&aws_config),
        table: config.table_name.clone(),
        bucket: config.notes_bucket.clone(),
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
    tracing::info!(table = %config.table_name, index = %config.vector_index, "starting indexer");

    lambda_runtime::run(service_fn(move |event: LambdaEvent<Value>| {
        let indexer = Arc::clone(&indexer);
        async move { indexer.handle(event.payload).await.map_err(Error::from) }
    }))
    .await
}

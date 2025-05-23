// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use anyhow::{Context, Result};
use clap::Parser;
use order_stream::Args;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    order_stream::run(&args).await.context("Running order-stream REST API failed")?;
    tracing::info!("order-stream REST API shutdown");

    Ok(())
}

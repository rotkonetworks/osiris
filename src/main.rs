#![recursion_limit = "256"]
mod trader;
pub use trader::Trader;

mod binance_fetcher;
pub use binance_fetcher::BinanceFetcher;

mod opt;
pub use opt::Opt;

mod wallet;
pub use wallet::Wallet;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    use clap::Parser;
    Opt::parse().exec().await
}

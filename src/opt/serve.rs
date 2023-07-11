use anyhow::Context;
use binance::{api::Binance, config::Config, market::*};
use clap::Parser;
use directories::ProjectDirs;
use futures::TryStreamExt;
use penumbra_custody::soft_kms::SoftKms;
use penumbra_proto::{
    custody::v1alpha1::{
        custody_protocol_service_client::CustodyProtocolServiceClient,
        custody_protocol_service_server::CustodyProtocolServiceServer,
    },
    view::v1alpha1::{
        view_protocol_service_client::ViewProtocolServiceClient,
        view_protocol_service_server::ViewProtocolServiceServer,
    },
};
use penumbra_view::{ViewClient, ViewService};
use std::path::PathBuf;
use url::Url;

use crate::{BinanceFetcher, Trader, Wallet};

#[derive(Debug, Clone, Parser)]
pub struct Serve {
    /// The transaction fee for each response (paid in upenumbra).
    #[structopt(long, default_value = "0")]
    fee: u64,
    /// Path to the directory to use to store data [default: platform appdata directory].
    #[clap(long, short)]
    data_dir: Option<PathBuf>,
    /// The URL of the pd gRPC endpoint on the remote node.
    #[clap(short, long, default_value = "http://testnet.penumbra.zone:8080")]
    node: Url,
    /// The source address index in the wallet to use when dispensing tokens (if unspecified uses
    /// any funds available).
    #[clap(long = "source", default_value = "0")]
    source_address: penumbra_keys::keys::AddressIndex,
    /// The websockets address to access the Binance API on.
    #[clap(long = "binance_ws", default_value = "wss://stream.binance.us:9443")]
    binance_ws: Url,
    /// The HTTP address to access the Binance REST API on.
    #[clap(long = "binance_rest", default_value = "https://api.binance.us")]
    binance_rest: Url,
    /// The symbols to find quotes between.
    symbols: Vec<String>,
}

impl Serve {
    pub async fn exec(self) -> anyhow::Result<()> {
        // Create all permutations between symbols and find the canonical ones
        // (i.e. the Binance API only knows about ETHBTC, not BTCETH).
        let permutations = self
            .symbols
            .iter()
            .flat_map(|s1| self.symbols.iter().map(move |s2| (s1, s2)))
            .filter(|(s1, s2)| s1 != s2)
            .map(|(s1, s2)| format!("{}{}", s1, s2))
            .collect::<Vec<_>>();

        let mut symbols = Vec::new();

        // the binance-rs library really hates trailing slashes here
        let mut binance_rest = String::from(self.binance_rest);
        if binance_rest.ends_with("/") {
            binance_rest.pop();
        }
        let mut binance_ws = String::from(self.binance_ws);
        if binance_ws.ends_with("/") {
            binance_ws.pop();
        }
        let binance_config = Config {
            rest_api_endpoint: binance_rest,
            ws_endpoint: binance_ws,
            ..Default::default()
        };

        let market: Market = Binance::new_with_config(None, None, &binance_config);

        // Find the permutations that Binance knows about.
        for permutation in permutations {
            match market.get_price(permutation.clone()) {
                Ok(_) => symbols.push(permutation.clone()),
                Err(_) => tracing::trace!("Invalid symbol: {}", permutation),
            }
        }

        tracing::debug!(?symbols, "found valid trading symbols in Binance API");

        // Look up the path to the view state file per platform, creating the directory if needed
        let data_dir = self.data_dir.unwrap_or_else(|| {
            ProjectDirs::from("zone", "penumbra", "pcli")
                .expect("can access penumbra project dir")
                .data_dir()
                .to_owned()
        });
        std::fs::create_dir_all(&data_dir).context("can create data dir")?;

        let custody_file = data_dir.clone().join("custody.json");

        // Build a custody service...
        let wallet =
            Wallet::load(custody_file).context("Failed to load wallet from local custody file")?;
        let soft_kms = SoftKms::new(wallet.spend_key.clone().into());
        let custody =
            CustodyProtocolServiceClient::new(CustodyProtocolServiceServer::new(soft_kms));

        let fvk = wallet.spend_key.full_viewing_key().clone();

        // Instantiate an in-memory view service.
        let view_storage =
            penumbra_view::Storage::load_or_initialize(None::<&str>, &fvk, self.node.clone())
                .await?;
        let view_service = ViewService::new(view_storage, self.node.clone()).await?;

        // Now build the view and custody clients, doing gRPC with ourselves
        let mut view = ViewProtocolServiceClient::new(ViewProtocolServiceServer::new(view_service));

        // Wait to synchronize the chain before doing anything else.
        tracing::info!(
            "starting initial sync: please wait for sync to complete before requesting tokens"
        );
        ViewClient::status_stream(&mut view, fvk.account_group_id())
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        // From this point on, the view service is synchronized.
        tracing::info!("initial sync complete");

        // Instantiate the trader (manages the bot's portfolio based on MPSC messages containing price quotes)
        let (quotes_sender, trader) =
            Trader::new(0, fvk, view, custody, symbols.clone(), self.node);

        // Instantiate the Binance fetcher (responsible for fetching binance API data and sending along to the trader)
        let binance_fetcher = BinanceFetcher::new(quotes_sender, symbols, binance_config);

        // Start the binance fetcher (responsible for fetching binance API data and sending along
        // the watch channel) and the trader (responsible for receiving quotes along the watch channel
        // and managing the bot's portfolio).
        tokio::select! {
            result = tokio::spawn(async move { binance_fetcher.run().await }) =>
                result.unwrap().context("error in binance fetcher service"),
            result = tokio::spawn(async move { trader.run().await }) =>
                result.unwrap().context("error in trader service"),
        }
    }
}

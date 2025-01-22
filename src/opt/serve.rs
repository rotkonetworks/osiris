use anyhow::Context;
use binance::{api::Binance, config::Config, market::*};
use camino::Utf8PathBuf;
use clap::Parser;
use directories::ProjectDirs;
use futures::TryStreamExt;
use penumbra_custody::soft_kms::SoftKms;
use penumbra_proto::{
    custody::v1::{
        custody_service_client::CustodyServiceClient, custody_service_server::CustodyServiceServer,
    },
    view::v1::{view_service_client::ViewServiceClient, view_service_server::ViewServiceServer},
};
use penumbra_view::{ViewClient, ViewServer};
use url::Url;

use crate::{BinanceFetcher, Trader, Wallet};

#[derive(Debug, Clone, Parser)]
#[clap(
    name = "osiris",
    about = "Osiris: An example Penumbra price replication LP bot.",
    version
)]
pub struct Serve {
    /// The transaction fee for each response (paid in upenumbra).
    #[structopt(long, default_value = "0")]
    fee: u64,
    /// The home directory used to store configuration and data.
    #[clap(long, default_value_t = default_home(), env = "PENUMBRA_PCLI_HOME")]
    home: Utf8PathBuf,
    /// The URL of the pd gRPC endpoint on the remote node.
    #[clap(short, long, default_value = "https://grpc.penumbra.silentvalidator.com/")]
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
        let permutations = self
            .symbols
            .iter()
            .flat_map(|s1| self.symbols.iter().map(move |s2| (s1, s2)))
            .filter(|(s1, s2)| s1 != s2)
            .map(|(s1, s2)| format!("{}{}", s2, s1))
            .collect::<Vec<_>>();

        // Configure binance endpoints
        let mut binance_rest = String::from(self.binance_rest);
        if binance_rest.ends_with('/') {
            binance_rest.pop();
        }
        let mut binance_ws = String::from(self.binance_ws);
        if binance_ws.ends_with('/') {
            binance_ws.pop();
        }

        let binance_config = Config {
            rest_api_endpoint: binance_rest.clone(),
            ws_endpoint: binance_ws,
            ..Default::default()
        };

        // Move the blocking market operations to a spawn_blocking task
        let symbols = {
            let permutations = permutations.clone();
            let config = binance_config.clone();
            tokio::task::spawn_blocking(move || {
                let market: Market = Binance::new_with_config(None, None, &config);
                permutations
                    .into_iter()
                    .filter_map(|permutation| {
                        match market.get_price(permutation.clone()) {
                            Ok(_) => {
                                tracing::trace!("Valid symbol: {}", permutation);
                                Some(permutation)
                            }
                            Err(_) => {
                                tracing::trace!("Invalid symbol: {}", permutation);
                                None
                            }
                        }
                    })
                .collect::<Vec<String>>()
            })
            .await?
        };

        tracing::debug!(?symbols, "found valid trading symbols in Binance API");

        let penumbra_config = tracing::debug_span!("penumbra-config").entered();
        tracing::debug!("importing wallet material");

        // Build a custody service...
        let pcli_config_file = self.home.join("config.toml");
        let wallet = Wallet::load(pcli_config_file)
            .context("failed to load wallet from local custody file")?;
        let soft_kms = SoftKms::new(wallet.spend_key.clone().into());
        let custody = CustodyServiceClient::new(CustodyServiceServer::new(soft_kms));

        let fvk = wallet.spend_key.full_viewing_key().clone();

        // Wait to synchronize the chain before doing anything else.
        tracing::info!(%self.node, "starting initial sync: ");
        // Instantiate an in-memory view service.
        let view_file = self.home.join("pcli-view.sqlite");
        let view_filepath = Some(view_file.to_string());
        let view_storage =
            penumbra_view::Storage::load_or_initialize(view_filepath, &fvk, self.node.clone())
                .await?;

        // Now build the view and custody clients, doing gRPC with ourselves
        let view_server = ViewServer::new(view_storage, self.node.clone()).await?;
        let mut view = ViewServiceClient::new(ViewServiceServer::new(view_server));

        ViewClient::status_stream(&mut view)
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        // From this point on, the view service is synchronized.
        tracing::info!(%self.node, "initial sync complete");
        penumbra_config.exit();

        let trader_config = tracing::debug_span!("trader-config").entered();
        // Instantiate the trader (manages the bot's portfolio based on MPSC messages containing price quotes)
        let (quotes_sender, trader) = Trader::new(
            self.source_address,
            fvk,
            view,
            custody,
            symbols.clone(),
            self.node,
        );
        trader_config.exit();

        let _binance_span = tracing::debug_span!("binance-fetcher").entered();
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

fn default_home() -> Utf8PathBuf {
    let path = ProjectDirs::from("zone", "penumbra", "pcli")
        .expect("Failed to get platform data dir")
        .data_dir()
        .to_path_buf();
    Utf8PathBuf::from_path_buf(path).expect("Platform default data dir was not UTF-8")
}

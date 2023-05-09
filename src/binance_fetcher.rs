use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::{pin::Pin, task::Poll};

use binance::model::BookTickerEvent;
use futures::{Future, FutureExt};
use penumbra_crypto::{memo::MemoPlaintext, Address, FullViewingKey, Value};
use penumbra_custody::{AuthorizeRequest, CustodyClient};
use penumbra_view::ViewClient;
use penumbra_wallet::plan::Planner;
use rand::rngs::OsRng;
use tokio::sync::watch::Sender;

use binance::config::Config;
use binance::websockets::WebSockets;
use binance::websockets::WebsocketEvent;

use crate::{trader::Request, PriceQuote};

/// The `Trader` maps `(DirectedTradingPair, Amount)` position requests to [`position::Id`] identifiers of opened positions.
pub struct BinanceFetcher {
    /// Sends quotes to the trader.
    txs: BTreeMap<String, Sender<Option<BookTickerEvent>>>,
    /// Keep track of last-sent prices to avoid spamming the trader.
    last_sent: BTreeMap<String, BookTickerEvent>,
    /// The symbols we are fetching.
    symbols: Vec<String>,
}

impl BinanceFetcher {
    pub fn new(
        txs: BTreeMap<String, Sender<Option<BookTickerEvent>>>,
        symbols: Vec<String>,
    ) -> Self {
        Self {
            txs,
            last_sent: BTreeMap::new(),
            symbols,
        }
    }

    /// Run the fetcher.
    pub async fn run(mut self) -> anyhow::Result<()> {
        tracing::debug!("running BinanceFetcher");

        let keep_running = AtomicBool::new(true); // Used to control the event loop
                                                  // let kline = format!("{}", "ethbtc@kline_1m");
                                                  // let book_ticker = format!("{}", "ethbtc@bookTicker");
        let endpoints = self
            .symbols
            .iter()
            .map(|symbol| format!("{}@bookTicker", symbol.to_lowercase()))
            .collect::<Vec<_>>();

        let mut web_socket = WebSockets::new(|event: WebsocketEvent| {
            match event {
                WebsocketEvent::BookTicker(book_ticker_event) => {
                    // Check if this quote has already been sent or not.
                    let last_sent_quote = self.last_sent.get(&book_ticker_event.symbol);

                    if last_sent_quote.is_none()
                        || (last_sent_quote.is_some()
                            && last_sent_quote.unwrap().update_id < book_ticker_event.update_id &&
                            // we actually only care to update iff the price has changed
                            (last_sent_quote.unwrap().best_bid != book_ticker_event.best_bid ||
                            last_sent_quote.unwrap().best_ask != book_ticker_event.best_ask))
                    {
                        tracing::debug!(?book_ticker_event, "received new quote");
                        self.txs
                            .get(&book_ticker_event.symbol)
                            .expect("missing sender for symbol")
                            .send(Some(book_ticker_event.clone()))
                            .expect("error sending price quote");
                        self.last_sent
                            .insert(book_ticker_event.symbol.clone(), book_ticker_event);
                    }
                }
                _ => (),
            };
            Ok(())
        });

        web_socket
            .connect_with_config(
                &format!("stream?streams={}", &endpoints.join("/")),
                &Config {
                    rest_api_endpoint: "https://api.binance.us".into(),
                    // ws_endpoint: "wss://stream.binance.us:9443/ws".into(),
                    ws_endpoint: "wss://stream.binance.us:9443".into(),
                    ..Default::default()
                },
            )
            .unwrap(); // check error
        if let Err(e) = web_socket.event_loop(&keep_running) {
            match e {
                err => {
                    println!("Error: {:?}", err);
                }
            }
        }
        web_socket.disconnect().unwrap();
        Ok(())
    }
}

// impl<V, C> tower::Service<(Address, Vec<Value>)> for BinanceFetcher<V, C>
// where
//     V: ViewClient + Clone + Send + 'static,
//     C: CustodyClient + Clone + Send + 'static,
// {
//     type Response = penumbra_transaction::Id;
//     type Error = anyhow::Error;
//     type Future =
//         Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

//     fn call(&mut self, req: (Address, Vec<Value>)) -> Self::Future {
//         let mut self2 = self.clone();
//         async move {
//             // 1. plan the transaction.
//             let (address, values) = req;
//             if values.is_empty() {
//                 return Err(anyhow::anyhow!(
//                     "tried to send empty list of values to address"
//                 ));
//             }
//             let mut planner = Planner::new(OsRng);
//             for value in values {
//                 planner.output(value, address);
//             }
//             planner
//                 .memo(MemoPlaintext {
//                     text: "Hello from Galileo, the Penumbra faucet bot".to_string(),
//                     sender: self2.fvk.payment_address(0.into()).0,
//                 })
//                 .unwrap();
//             let plan = planner.plan(
//                 &mut self2.view,
//                 self2.fvk.account_group_id(),
//                 self2.account.into(),
//             );
//             let plan = plan.await?;

//             // 2. Authorize and build the transaction.
//             let auth_data = self2
//                 .custody
//                 .authorize(AuthorizeRequest {
//                     plan: plan.clone(),
//                     account_group_id: Some(self2.fvk.account_group_id()),
//                     pre_authorizations: Vec::new(),
//                 })
//                 .await?
//                 .data
//                 .ok_or_else(|| anyhow::anyhow!("no auth data"))?
//                 .try_into()?;
//             let witness_data = self2
//                 .view
//                 .witness(self2.fvk.account_group_id(), &plan)
//                 .await?;
//             let unauth_tx = plan
//                 .build_concurrent(OsRng, &self2.fvk, witness_data)
//                 .await?;

//             let tx = unauth_tx.authorize(&mut OsRng, &auth_data)?;

//             // 3. Broadcast the transaction and wait for confirmation.
//             self2.view.broadcast_transaction(tx, true).await
//         }
//         .boxed()
//     }

//     fn poll_ready(
//         &mut self,
//         _cx: &mut std::task::Context<'_>,
//     ) -> std::task::Poll<Result<(), Self::Error>> {
//         // We always return "ready" because we handle backpressure by making the
//         // constructor return a concurrency limit wrapper.
//         Poll::Ready(Ok(()))
//     }
// }
